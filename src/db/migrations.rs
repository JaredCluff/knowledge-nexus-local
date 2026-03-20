use anyhow::{Context, Result};
use rusqlite::Connection;
use tracing::info;

const CURRENT_VERSION: i32 = 3;

pub fn run_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)")
        .context("Failed to create schema_version table")?;

    let version: i32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if version < 1 {
        info!("Running migration v1: initial schema");
        migrate_v1(conn)?;
        conn.execute("INSERT INTO schema_version (version) VALUES (?1)", [1])?;
    }

    if version < 2 {
        info!("Running migration v2: FTS5 full-text search on articles");
        migrate_v2(conn)?;
        conn.execute("INSERT INTO schema_version (version) VALUES (?1)", [2])?;
    }

    if version < 3 {
        info!("Running migration v3: add status column to k2k_clients");
        migrate_v3(conn)?;
        conn.execute("INSERT INTO schema_version (version) VALUES (?1)", [3])?;
    }

    info!("Database schema at version {}", CURRENT_VERSION);
    Ok(())
}

fn migrate_v1(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY NOT NULL,
            username TEXT NOT NULL UNIQUE,
            display_name TEXT NOT NULL,
            is_owner INTEGER NOT NULL DEFAULT 0,
            settings TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS knowledge_stores (
            id TEXT PRIMARY KEY NOT NULL,
            owner_id TEXT NOT NULL,
            store_type TEXT NOT NULL CHECK(store_type IN ('personal', 'family', 'shared')),
            name TEXT NOT NULL,
            lancedb_collection TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (owner_id) REFERENCES users(id)
        );

        CREATE TABLE IF NOT EXISTS articles (
            id TEXT PRIMARY KEY NOT NULL,
            store_id TEXT NOT NULL,
            title TEXT NOT NULL,
            content TEXT NOT NULL,
            source_type TEXT NOT NULL DEFAULT 'user',
            tags TEXT NOT NULL DEFAULT '[]',
            embedded_at TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (store_id) REFERENCES knowledge_stores(id)
        );

        CREATE TABLE IF NOT EXISTS conversations (
            id TEXT PRIMARY KEY NOT NULL,
            user_id TEXT NOT NULL,
            title TEXT NOT NULL DEFAULT 'New Conversation',
            message_count INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (user_id) REFERENCES users(id)
        );

        CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY NOT NULL,
            conversation_id TEXT NOT NULL,
            role TEXT NOT NULL CHECK(role IN ('user', 'assistant', 'system')),
            content TEXT NOT NULL,
            metadata TEXT NOT NULL DEFAULT '{}',
            created_at TEXT NOT NULL,
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS k2k_clients (
            client_id TEXT PRIMARY KEY NOT NULL,
            public_key_pem TEXT NOT NULL,
            client_name TEXT NOT NULL DEFAULT '',
            registered_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS federation_agreements (
            id TEXT PRIMARY KEY NOT NULL,
            local_store_id TEXT NOT NULL,
            remote_node_id TEXT NOT NULL,
            remote_endpoint TEXT NOT NULL,
            access_type TEXT NOT NULL DEFAULT 'read' CHECK(access_type IN ('read', 'write', 'readwrite')),
            created_at TEXT NOT NULL,
            FOREIGN KEY (local_store_id) REFERENCES knowledge_stores(id)
        );

        CREATE TABLE IF NOT EXISTS discovered_nodes (
            node_id TEXT PRIMARY KEY NOT NULL,
            host TEXT NOT NULL,
            port INTEGER NOT NULL,
            endpoint TEXT NOT NULL,
            capabilities TEXT NOT NULL DEFAULT '[]',
            last_seen TEXT NOT NULL,
            healthy INTEGER NOT NULL DEFAULT 1
        );

        CREATE TABLE IF NOT EXISTS connector_configs (
            id TEXT PRIMARY KEY NOT NULL,
            connector_type TEXT NOT NULL,
            name TEXT NOT NULL,
            config TEXT NOT NULL DEFAULT '{}',
            store_id TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            FOREIGN KEY (store_id) REFERENCES knowledge_stores(id)
        );

        CREATE INDEX IF NOT EXISTS idx_articles_store_id ON articles(store_id);
        CREATE INDEX IF NOT EXISTS idx_messages_conversation_id ON messages(conversation_id);
        CREATE INDEX IF NOT EXISTS idx_conversations_user_id ON conversations(user_id);
        CREATE INDEX IF NOT EXISTS idx_knowledge_stores_owner_id ON knowledge_stores(owner_id);
        CREATE INDEX IF NOT EXISTS idx_federation_agreements_local_store ON federation_agreements(local_store_id);
        CREATE INDEX IF NOT EXISTS idx_connector_configs_store_id ON connector_configs(store_id);
        "
    )
    .context("Failed to run migration v1")?;

    Ok(())
}

fn migrate_v2(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        -- FTS5 virtual table for full-text search on articles
        CREATE VIRTUAL TABLE IF NOT EXISTS articles_fts USING fts5(
            title,
            content,
            tags,
            content='articles',
            content_rowid='rowid'
        );

        -- Triggers to keep FTS index in sync with articles table
        CREATE TRIGGER IF NOT EXISTS articles_fts_insert AFTER INSERT ON articles BEGIN
            INSERT INTO articles_fts(rowid, title, content, tags) VALUES (new.rowid, new.title, new.content, new.tags);
        END;

        CREATE TRIGGER IF NOT EXISTS articles_fts_delete AFTER DELETE ON articles BEGIN
            INSERT INTO articles_fts(articles_fts, rowid, title, content, tags) VALUES ('delete', old.rowid, old.title, old.content, old.tags);
        END;

        CREATE TRIGGER IF NOT EXISTS articles_fts_update AFTER UPDATE ON articles BEGIN
            INSERT INTO articles_fts(articles_fts, rowid, title, content, tags) VALUES ('delete', old.rowid, old.title, old.content, old.tags);
            INSERT INTO articles_fts(rowid, title, content, tags) VALUES (new.rowid, new.title, new.content, new.tags);
        END;

        -- Rebuild FTS index from existing articles
        INSERT INTO articles_fts(articles_fts) VALUES ('rebuild');
        "
    )
    .context("Failed to run migration v2 (FTS5)")?;

    Ok(())
}

fn migrate_v3(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "ALTER TABLE k2k_clients ADD COLUMN status TEXT NOT NULL DEFAULT 'approved';",
    )
    .context("Failed to run migration v3 (k2k_clients status)")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_migrations_run_clean() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        run_migrations(&conn)?;

        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?
            .query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        assert!(tables.contains(&"users".to_string()));
        assert!(tables.contains(&"knowledge_stores".to_string()));
        assert!(tables.contains(&"articles".to_string()));
        assert!(tables.contains(&"conversations".to_string()));
        assert!(tables.contains(&"messages".to_string()));
        assert!(tables.contains(&"k2k_clients".to_string()));
        assert!(tables.contains(&"federation_agreements".to_string()));
        assert!(tables.contains(&"discovered_nodes".to_string()));
        assert!(tables.contains(&"connector_configs".to_string()));
        Ok(())
    }

    #[test]
    fn test_migrations_idempotent() -> Result<()> {
        let conn = Connection::open_in_memory()?;
        run_migrations(&conn)?;
        run_migrations(&conn)?; // Should not fail
        Ok(())
    }
}
