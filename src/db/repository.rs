use std::path::Path;
use std::sync::Mutex;

use anyhow::{Context, Result};
use rusqlite::params;
use tracing::{info, warn};

use super::migrations;
use super::models::*;

pub struct Database {
    conn: Mutex<rusqlite::Connection>,
}

impl Database {
    /// Acquire the database connection lock.
    /// Recovers from a poisoned mutex (caused by a panic inside a lock guard)
    /// so a single panicking request doesn't permanently disable the database.
    fn conn(&self) -> Result<std::sync::MutexGuard<'_, rusqlite::Connection>> {
        self.conn.lock().or_else(|poison| {
            warn!("Database mutex was poisoned; recovering");
            Ok(poison.into_inner())
        })
    }

    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = rusqlite::Connection::open(path)
            .with_context(|| format!("Failed to open SQLite database at {:?}", path))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
        migrations::run_migrations(&conn)?;

        // Restrict database file to owner-only (0o600) so other local users cannot
        // read indexed file content, conversations, and client metadata stored in it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("Failed to set database file permissions at {:?}", path))?;
        }

        info!("Database opened at {:?}", path);
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    #[allow(dead_code)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = rusqlite::Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        migrations::run_migrations(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    // ========================================================================
    // Users
    // ========================================================================

    pub fn create_user(&self, user: &User) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO users (id, username, display_name, is_owner, settings, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                user.id,
                user.username,
                user.display_name,
                user.is_owner as i32,
                user.settings.to_string(),
                user.created_at,
                user.updated_at,
            ],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn get_user(&self, id: &str) -> Result<Option<User>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, username, display_name, is_owner, settings, created_at, updated_at FROM users WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(User {
                id: row.get(0)?,
                username: row.get(1)?,
                display_name: row.get(2)?,
                is_owner: row.get::<_, i32>(3)? != 0,
                settings: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;
        match rows.next() {
            Some(Ok(user)) => Ok(Some(user)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    pub fn get_owner_user(&self) -> Result<Option<User>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, username, display_name, is_owner, settings, created_at, updated_at FROM users WHERE is_owner = 1 LIMIT 1",
        )?;
        let mut rows = stmt.query_map([], |row| {
            Ok(User {
                id: row.get(0)?,
                username: row.get(1)?,
                display_name: row.get(2)?,
                is_owner: true,
                settings: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;
        match rows.next() {
            Some(Ok(user)) => Ok(Some(user)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    #[allow(dead_code)]
    pub fn list_users(&self) -> Result<Vec<User>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, username, display_name, is_owner, settings, created_at, updated_at FROM users ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(User {
                id: row.get(0)?,
                username: row.get(1)?,
                display_name: row.get(2)?,
                is_owner: row.get::<_, i32>(3)? != 0,
                settings: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    // ========================================================================
    // Knowledge Stores
    // ========================================================================

    pub fn create_store(&self, store: &KnowledgeStore) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO knowledge_stores (id, owner_id, store_type, name, lancedb_collection, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                store.id,
                store.owner_id,
                store.store_type,
                store.name,
                store.lancedb_collection,
                store.created_at,
                store.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_store(&self, id: &str) -> Result<Option<KnowledgeStore>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, owner_id, store_type, name, lancedb_collection, created_at, updated_at FROM knowledge_stores WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(KnowledgeStore {
                id: row.get(0)?,
                owner_id: row.get(1)?,
                store_type: row.get(2)?,
                name: row.get(3)?,
                lancedb_collection: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;
        match rows.next() {
            Some(Ok(store)) => Ok(Some(store)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    pub fn list_stores(&self) -> Result<Vec<KnowledgeStore>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, owner_id, store_type, name, lancedb_collection, created_at, updated_at FROM knowledge_stores ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(KnowledgeStore {
                id: row.get(0)?,
                owner_id: row.get(1)?,
                store_type: row.get(2)?,
                name: row.get(3)?,
                lancedb_collection: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn list_stores_for_user(&self, owner_id: &str) -> Result<Vec<KnowledgeStore>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, owner_id, store_type, name, lancedb_collection, created_at, updated_at
             FROM knowledge_stores WHERE owner_id = ?1 ORDER BY created_at",
        )?;
        let rows = stmt.query_map(params![owner_id], |row| {
            Ok(KnowledgeStore {
                id: row.get(0)?,
                owner_id: row.get(1)?,
                store_type: row.get(2)?,
                name: row.get(3)?,
                lancedb_collection: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    // ========================================================================
    // Articles
    // ========================================================================

    pub fn create_article(&self, article: &Article) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO articles (id, store_id, title, content, source_type, tags, embedded_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                article.id,
                article.store_id,
                article.title,
                article.content,
                article.source_type,
                article.tags.to_string(),
                article.embedded_at,
                article.created_at,
                article.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_article(&self, id: &str) -> Result<Option<Article>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, store_id, title, content, source_type, tags, embedded_at, created_at, updated_at FROM articles WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(Article {
                id: row.get(0)?,
                store_id: row.get(1)?,
                title: row.get(2)?,
                content: row.get(3)?,
                source_type: row.get(4)?,
                tags: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or_default(),
                embedded_at: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;
        match rows.next() {
            Some(Ok(article)) => Ok(Some(article)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    pub fn update_article(&self, article: &Article) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE articles SET title = ?1, content = ?2, source_type = ?3, tags = ?4, embedded_at = ?5, updated_at = ?6 WHERE id = ?7",
            params![
                article.title,
                article.content,
                article.source_type,
                article.tags.to_string(),
                article.embedded_at,
                article.updated_at,
                article.id,
            ],
        )?;
        Ok(())
    }

    pub fn delete_article(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM articles WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Count the total number of articles across all stores owned by `owner_id`.
    ///
    /// Used for per-client storage quota enforcement in the K2K task queue.
    pub fn count_articles_for_owner(&self, owner_id: &str) -> Result<usize> {
        let conn = self.conn()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM articles a
             INNER JOIN knowledge_stores s ON a.store_id = s.id
             WHERE s.owner_id = ?1",
            params![owner_id],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    pub fn list_articles_for_store(&self, store_id: &str) -> Result<Vec<Article>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, store_id, title, content, source_type, tags, embedded_at, created_at, updated_at
             FROM articles WHERE store_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![store_id], |row| {
            Ok(Article {
                id: row.get(0)?,
                store_id: row.get(1)?,
                title: row.get(2)?,
                content: row.get(3)?,
                source_type: row.get(4)?,
                tags: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or_default(),
                embedded_at: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    // ========================================================================
    // Conversations
    // ========================================================================

    pub fn create_conversation(&self, conv: &Conversation) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO conversations (id, user_id, title, message_count, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                conv.id,
                conv.user_id,
                conv.title,
                conv.message_count,
                conv.created_at,
                conv.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn get_conversation(&self, id: &str) -> Result<Option<Conversation>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, user_id, title, message_count, created_at, updated_at FROM conversations WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(Conversation {
                id: row.get(0)?,
                user_id: row.get(1)?,
                title: row.get(2)?,
                message_count: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?;
        match rows.next() {
            Some(Ok(conv)) => Ok(Some(conv)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    pub fn list_conversations_for_user(&self, user_id: &str) -> Result<Vec<Conversation>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, user_id, title, message_count, created_at, updated_at
             FROM conversations WHERE user_id = ?1 ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map(params![user_id], |row| {
            Ok(Conversation {
                id: row.get(0)?,
                user_id: row.get(1)?,
                title: row.get(2)?,
                message_count: row.get(3)?,
                created_at: row.get(4)?,
                updated_at: row.get(5)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    // ========================================================================
    // Messages
    // ========================================================================

    pub fn create_message(&self, msg: &Message) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO messages (id, conversation_id, role, content, metadata, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                msg.id,
                msg.conversation_id,
                msg.role,
                msg.content,
                msg.metadata.to_string(),
                msg.created_at,
            ],
        )?;
        // Increment message_count
        conn.execute(
            "UPDATE conversations SET message_count = message_count + 1, updated_at = ?1 WHERE id = ?2",
            params![msg.created_at, msg.conversation_id],
        )?;
        Ok(())
    }

    pub fn list_messages_for_conversation(&self, conversation_id: &str) -> Result<Vec<Message>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, role, content, metadata, created_at
             FROM messages WHERE conversation_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![conversation_id], |row| {
            Ok(Message {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                role: row.get(2)?,
                content: row.get(3)?,
                metadata: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
                created_at: row.get(5)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    // ========================================================================
    // K2K Clients
    // ========================================================================

    pub fn upsert_k2k_client(&self, client: &K2KClient) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO k2k_clients (client_id, public_key_pem, client_name, registered_at, status)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                client.client_id,
                client.public_key_pem,
                client.client_name,
                client.registered_at,
                client.status,
            ],
        )?;
        Ok(())
    }

    pub fn get_k2k_client(&self, client_id: &str) -> Result<Option<K2KClient>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT client_id, public_key_pem, client_name, registered_at, status FROM k2k_clients WHERE client_id = ?1",
        )?;
        let mut rows = stmt.query_map(params![client_id], |row| {
            Ok(K2KClient {
                client_id: row.get(0)?,
                public_key_pem: row.get(1)?,
                client_name: row.get(2)?,
                registered_at: row.get(3)?,
                status: row.get(4)?,
            })
        })?;
        match rows.next() {
            Some(Ok(client)) => Ok(Some(client)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    pub fn list_k2k_clients(&self) -> Result<Vec<K2KClient>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT client_id, public_key_pem, client_name, registered_at, status FROM k2k_clients ORDER BY registered_at",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(K2KClient {
                client_id: row.get(0)?,
                public_key_pem: row.get(1)?,
                client_name: row.get(2)?,
                registered_at: row.get(3)?,
                status: row.get(4)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn list_pending_k2k_clients(&self) -> Result<Vec<K2KClient>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT client_id, public_key_pem, client_name, registered_at, status FROM k2k_clients WHERE status = 'pending' ORDER BY registered_at",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(K2KClient {
                client_id: row.get(0)?,
                public_key_pem: row.get(1)?,
                client_name: row.get(2)?,
                registered_at: row.get(3)?,
                status: row.get(4)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn update_k2k_client_status(&self, client_id: &str, status: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE k2k_clients SET status = ?1 WHERE client_id = ?2",
            params![status, client_id],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn delete_k2k_client(&self, client_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM k2k_clients WHERE client_id = ?1",
            params![client_id],
        )?;
        Ok(())
    }

    // ========================================================================
    // Federation Agreements
    // ========================================================================

    pub fn create_federation_agreement(&self, agreement: &FederationAgreement) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO federation_agreements (id, local_store_id, remote_node_id, remote_endpoint, access_type, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                agreement.id,
                agreement.local_store_id,
                agreement.remote_node_id,
                agreement.remote_endpoint,
                agreement.access_type,
                agreement.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn list_federation_agreements(&self) -> Result<Vec<FederationAgreement>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, local_store_id, remote_node_id, remote_endpoint, access_type, created_at
             FROM federation_agreements ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(FederationAgreement {
                id: row.get(0)?,
                local_store_id: row.get(1)?,
                remote_node_id: row.get(2)?,
                remote_endpoint: row.get(3)?,
                access_type: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    #[allow(dead_code)]
    pub fn delete_federation_agreement(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM federation_agreements WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    // ========================================================================
    // Discovered Nodes
    // ========================================================================

    pub fn upsert_discovered_node(&self, node: &DiscoveredNode) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO discovered_nodes (node_id, host, port, endpoint, capabilities, last_seen, healthy)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                node.node_id,
                node.host,
                node.port,
                node.endpoint,
                node.capabilities.to_string(),
                node.last_seen,
                node.healthy as i32,
            ],
        )?;
        Ok(())
    }

    pub fn list_discovered_nodes(&self) -> Result<Vec<DiscoveredNode>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT node_id, host, port, endpoint, capabilities, last_seen, healthy FROM discovered_nodes ORDER BY last_seen DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(DiscoveredNode {
                node_id: row.get(0)?,
                host: row.get(1)?,
                port: {
                    let p: i32 = row.get(2)?;
                    if !(1..=65535).contains(&p) {
                        return Err(rusqlite::Error::IntegralValueOutOfRange(2, p as i64));
                    }
                    p as u16
                },
                endpoint: row.get(3)?,
                capabilities: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
                last_seen: row.get(5)?,
                healthy: row.get::<_, i32>(6)? != 0,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn mark_node_unhealthy(&self, node_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE discovered_nodes SET healthy = 0 WHERE node_id = ?1",
            params![node_id],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn delete_discovered_node(&self, node_id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM discovered_nodes WHERE node_id = ?1",
            params![node_id],
        )?;
        Ok(())
    }

    // ========================================================================
    // Connector Configs
    // ========================================================================

    #[allow(dead_code)]
    pub fn create_connector_config(&self, config: &ConnectorConfig) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO connector_configs (id, connector_type, name, config, store_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                config.id,
                config.connector_type,
                config.name,
                config.config.to_string(),
                config.store_id,
                config.created_at,
                config.updated_at,
            ],
        )?;
        Ok(())
    }

    #[allow(dead_code)]
    pub fn list_connector_configs(&self) -> Result<Vec<ConnectorConfig>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, connector_type, name, config, store_id, created_at, updated_at FROM connector_configs ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ConnectorConfig {
                id: row.get(0)?,
                connector_type: row.get(1)?,
                name: row.get(2)?,
                config: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or_default(),
                store_id: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    #[allow(dead_code)]
    pub fn delete_connector_config(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM connector_configs WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ========================================================================
    // Full-Text Search
    // ========================================================================

    /// Search articles using FTS5 full-text search
    pub fn fts_search_articles(&self, query: &str, limit: usize) -> Result<Vec<Article>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT a.id, a.store_id, a.title, a.content, a.source_type, a.tags, a.embedded_at, a.created_at, a.updated_at
             FROM articles a
             JOIN articles_fts fts ON a.rowid = fts.rowid
             WHERE articles_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = stmt.query_map(params![query, limit_i64], |row| {
            Ok(Article {
                id: row.get(0)?,
                store_id: row.get(1)?,
                title: row.get(2)?,
                content: row.get(3)?,
                source_type: row.get(4)?,
                tags: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or_default(),
                embedded_at: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    fn now() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    fn test_db() -> Result<Database> {
        Database::open_in_memory()
    }

    #[test]
    fn test_user_crud() -> Result<()> {
        let db = test_db()?;
        let ts = now();
        let user = User {
            id: "u1".into(),
            username: "alice".into(),
            display_name: "Alice".into(),
            is_owner: true,
            settings: serde_json::json!({}),
            created_at: ts.clone(),
            updated_at: ts,
        };
        db.create_user(&user)?;

        let fetched = db.get_user("u1")?.expect("user should exist");
        assert_eq!(fetched.username, "alice");
        assert!(fetched.is_owner);

        let owner = db.get_owner_user()?.expect("owner should exist");
        assert_eq!(owner.id, "u1");

        let users = db.list_users()?;
        assert_eq!(users.len(), 1);
        Ok(())
    }

    #[test]
    fn test_store_crud() -> Result<()> {
        let db = test_db()?;
        let ts = now();
        let user = User {
            id: "u1".into(),
            username: "alice".into(),
            display_name: "Alice".into(),
            is_owner: true,
            settings: serde_json::json!({}),
            created_at: ts.clone(),
            updated_at: ts.clone(),
        };
        db.create_user(&user)?;

        let store = KnowledgeStore {
            id: "s1".into(),
            owner_id: "u1".into(),
            store_type: "personal".into(),
            name: "Alice's Notes".into(),
            lancedb_collection: "store_s1".into(),
            created_at: ts.clone(),
            updated_at: ts,
        };
        db.create_store(&store)?;

        let fetched = db.get_store("s1")?.expect("store should exist");
        assert_eq!(fetched.name, "Alice's Notes");

        let stores = db.list_stores()?;
        assert_eq!(stores.len(), 1);

        let user_stores = db.list_stores_for_user("u1")?;
        assert_eq!(user_stores.len(), 1);
        Ok(())
    }

    #[test]
    fn test_article_crud() -> Result<()> {
        let db = test_db()?;
        let ts = now();
        db.create_user(&User {
            id: "u1".into(),
            username: "alice".into(),
            display_name: "Alice".into(),
            is_owner: true,
            settings: serde_json::json!({}),
            created_at: ts.clone(),
            updated_at: ts.clone(),
        })?;
        db.create_store(&KnowledgeStore {
            id: "s1".into(),
            owner_id: "u1".into(),
            store_type: "personal".into(),
            name: "Notes".into(),
            lancedb_collection: "store_s1".into(),
            created_at: ts.clone(),
            updated_at: ts.clone(),
        })?;

        let article = Article {
            id: "a1".into(),
            store_id: "s1".into(),
            title: "Test Article".into(),
            content: "Hello world".into(),
            source_type: "user".into(),
            tags: serde_json::json!(["test"]),
            embedded_at: None,
            created_at: ts.clone(),
            updated_at: ts.clone(),
        };
        db.create_article(&article)?;

        let fetched = db.get_article("a1")?.expect("article should exist");
        assert_eq!(fetched.title, "Test Article");

        let mut updated = fetched.clone();
        updated.title = "Updated Title".into();
        updated.updated_at = now();
        db.update_article(&updated)?;

        let fetched2 = db.get_article("a1")?.expect("article should exist");
        assert_eq!(fetched2.title, "Updated Title");

        let articles = db.list_articles_for_store("s1")?;
        assert_eq!(articles.len(), 1);

        db.delete_article("a1")?;
        assert!(db.get_article("a1")?.is_none());
        Ok(())
    }

    #[test]
    fn test_conversation_and_messages() -> Result<()> {
        let db = test_db()?;
        let ts = now();
        db.create_user(&User {
            id: "u1".into(),
            username: "alice".into(),
            display_name: "Alice".into(),
            is_owner: true,
            settings: serde_json::json!({}),
            created_at: ts.clone(),
            updated_at: ts.clone(),
        })?;

        let conv = Conversation {
            id: "c1".into(),
            user_id: "u1".into(),
            title: "Test Chat".into(),
            message_count: 0,
            created_at: ts.clone(),
            updated_at: ts.clone(),
        };
        db.create_conversation(&conv)?;

        let msg = Message {
            id: "m1".into(),
            conversation_id: "c1".into(),
            role: "user".into(),
            content: "Hello".into(),
            metadata: serde_json::json!({}),
            created_at: ts.clone(),
        };
        db.create_message(&msg)?;

        let conv = db
            .get_conversation("c1")?
            .expect("conversation should exist");
        assert_eq!(conv.message_count, 1);

        let messages = db.list_messages_for_conversation("c1")?;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content, "Hello");

        let convs = db.list_conversations_for_user("u1")?;
        assert_eq!(convs.len(), 1);
        Ok(())
    }

    #[test]
    fn test_k2k_client_crud() -> Result<()> {
        let db = test_db()?;
        let client = K2KClient {
            client_id: "client1".into(),
            public_key_pem: "-----BEGIN PUBLIC KEY-----\ntest\n-----END PUBLIC KEY-----".into(),
            client_name: "Test Client".into(),
            registered_at: now(),
            status: "approved".into(),
        };
        db.upsert_k2k_client(&client)?;

        let fetched = db.get_k2k_client("client1")?.expect("client should exist");
        assert_eq!(fetched.client_name, "Test Client");

        let clients = db.list_k2k_clients()?;
        assert_eq!(clients.len(), 1);

        // Upsert should update
        let mut updated = client.clone();
        updated.client_name = "Updated Client".into();
        db.upsert_k2k_client(&updated)?;
        let fetched2 = db.get_k2k_client("client1")?.expect("client should exist");
        assert_eq!(fetched2.client_name, "Updated Client");

        db.delete_k2k_client("client1")?;
        assert!(db.get_k2k_client("client1")?.is_none());
        Ok(())
    }

    #[test]
    fn test_discovered_nodes() -> Result<()> {
        let db = test_db()?;
        let node = DiscoveredNode {
            node_id: "node1".into(),
            host: "192.168.1.10".into(),
            port: 8765,
            endpoint: "http://192.168.1.10:8765/k2k/v1".into(),
            capabilities: serde_json::json!(["semantic_search"]),
            last_seen: now(),
            healthy: true,
        };
        db.upsert_discovered_node(&node)?;

        let nodes = db.list_discovered_nodes()?;
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].healthy);

        db.mark_node_unhealthy("node1")?;
        let nodes2 = db.list_discovered_nodes()?;
        assert!(!nodes2[0].healthy);

        db.delete_discovered_node("node1")?;
        assert!(db.list_discovered_nodes()?.is_empty());
        Ok(())
    }

    #[test]
    fn test_federation_agreements() -> Result<()> {
        let db = test_db()?;
        let ts = now();
        db.create_user(&User {
            id: "u1".into(),
            username: "alice".into(),
            display_name: "Alice".into(),
            is_owner: true,
            settings: serde_json::json!({}),
            created_at: ts.clone(),
            updated_at: ts.clone(),
        })?;
        db.create_store(&KnowledgeStore {
            id: "s1".into(),
            owner_id: "u1".into(),
            store_type: "personal".into(),
            name: "Notes".into(),
            lancedb_collection: "store_s1".into(),
            created_at: ts.clone(),
            updated_at: ts.clone(),
        })?;

        let agreement = FederationAgreement {
            id: "fa1".into(),
            local_store_id: "s1".into(),
            remote_node_id: "node2".into(),
            remote_endpoint: "http://192.168.1.20:8765/k2k/v1".into(),
            access_type: "read".into(),
            created_at: ts,
        };
        db.create_federation_agreement(&agreement)?;

        let agreements = db.list_federation_agreements()?;
        assert_eq!(agreements.len(), 1);

        db.delete_federation_agreement("fa1")?;
        assert!(db.list_federation_agreements()?.is_empty());
        Ok(())
    }

    #[test]
    fn test_connector_configs() -> Result<()> {
        let db = test_db()?;
        let ts = now();
        db.create_user(&User {
            id: "u1".into(),
            username: "alice".into(),
            display_name: "Alice".into(),
            is_owner: true,
            settings: serde_json::json!({}),
            created_at: ts.clone(),
            updated_at: ts.clone(),
        })?;
        db.create_store(&KnowledgeStore {
            id: "s1".into(),
            owner_id: "u1".into(),
            store_type: "personal".into(),
            name: "Notes".into(),
            lancedb_collection: "store_s1".into(),
            created_at: ts.clone(),
            updated_at: ts.clone(),
        })?;

        let cc = ConnectorConfig {
            id: "cc1".into(),
            connector_type: "local_files".into(),
            name: "My Documents".into(),
            config: serde_json::json!({"path": "/home/docs"}),
            store_id: "s1".into(),
            created_at: ts.clone(),
            updated_at: ts,
        };
        db.create_connector_config(&cc)?;

        let configs = db.list_connector_configs()?;
        assert_eq!(configs.len(), 1);

        db.delete_connector_config("cc1")?;
        assert!(db.list_connector_configs()?.is_empty());
        Ok(())
    }
}
