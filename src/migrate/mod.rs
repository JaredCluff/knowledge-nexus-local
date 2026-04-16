//! One-shot migrator: 0.8 SQLite -> 1.0.0 SurrealDB.
//!
//! Invoked via `knowledge-nexus-agent migrate --from sqlite --to surrealdb`.
//! Idempotent at the row level: every write is an upsert keyed on the legacy
//! primary key. A failure mid-run leaves the target DB partially populated
//! but NO `migration_completed` marker -- `start` will refuse to open it,
//! and `--force` is needed to retry.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use tracing::info;

use crate::store::{
    hash, Conversation, DiscoveredNode, FederationAgreement,
    K2KClient, KnowledgeStore, Store, SurrealStore,
};

pub const MIGRATION_MARKER_FILENAME: &str = "migration_completed";

#[derive(Debug, Default)]
pub struct MigrationReport {
    pub users: usize,
    pub stores: usize,
    pub articles: usize,
    pub conversations: usize,
    pub messages: usize,
    pub k2k_clients: usize,
    pub federation_agreements: usize,
    pub discovered_nodes: usize,
    pub connector_configs: usize,
}

pub fn migration_marker_path(surreal_dir: &Path) -> PathBuf {
    surreal_dir.join(MIGRATION_MARKER_FILENAME)
}

pub fn is_migrated(surreal_dir: &Path) -> bool {
    migration_marker_path(surreal_dir).exists()
}

pub async fn migrate(
    sqlite_path: &Path,
    surreal_dir: &Path,
    force: bool,
) -> Result<MigrationReport> {
    if is_migrated(surreal_dir) && !force {
        bail!(
            "migration_completed marker already present at {:?}. \
             Use --force to re-run the migration (idempotent upserts).",
            migration_marker_path(surreal_dir)
        );
    }

    info!(
        "Migrating 0.8 SQLite {:?} -> 1.0.0 SurrealDB {:?}",
        sqlite_path, surreal_dir
    );

    let sqlite = Connection::open_with_flags(
        sqlite_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .with_context(|| format!("Failed to open legacy SQLite at {:?}", sqlite_path))?;

    let store = SurrealStore::open(surreal_dir).await?;

    let mut report = MigrationReport::default();
    report.users = migrate_users(&sqlite, &store).await?;
    report.stores = migrate_stores(&sqlite, &store).await?;
    report.articles = migrate_articles(&sqlite, &store).await?;
    report.conversations = migrate_conversations(&sqlite, &store).await?;
    report.messages = migrate_messages(&sqlite, &store).await?;
    report.k2k_clients = migrate_k2k_clients(&sqlite, &store).await?;
    report.federation_agreements = migrate_federation_agreements(&sqlite, &store).await?;
    report.discovered_nodes = migrate_discovered_nodes(&sqlite, &store).await?;
    report.connector_configs = migrate_connector_configs(&sqlite, &store).await?;

    std::fs::write(
        migration_marker_path(surreal_dir),
        chrono::Utc::now().to_rfc3339(),
    )
    .context("Failed to write migration_completed marker")?;

    info!(
        "Migration complete: {} users, {} stores, {} articles, {} conversations, \
         {} messages, {} k2k_clients, {} federation_agreements, {} discovered_nodes, \
         {} connector_configs",
        report.users,
        report.stores,
        report.articles,
        report.conversations,
        report.messages,
        report.k2k_clients,
        report.federation_agreements,
        report.discovered_nodes,
        report.connector_configs,
    );
    Ok(report)
}

// ---------------------------------------------------------------------------
// Per-table migration functions
// ---------------------------------------------------------------------------

async fn migrate_users(sqlite: &Connection, store: &SurrealStore) -> Result<usize> {
    let mut stmt = sqlite.prepare(
        "SELECT id, username, display_name, is_owner, settings, created_at, updated_at FROM users",
    )?;

    let rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let username: String = row.get(1)?;
        let display_name: String = row.get(2)?;
        let is_owner_int: i64 = row.get(3)?;
        let settings_str: String = row.get(4)?;
        let created_at: String = row.get(5)?;
        let updated_at: String = row.get(6)?;
        Ok((id, username, display_name, is_owner_int, settings_str, created_at, updated_at))
    })?;

    let mut count = 0usize;
    for row in rows {
        let (id, username, display_name, is_owner_int, settings_str, created_at, updated_at) =
            row.context("Failed to read user row from SQLite")?;
        let is_owner = is_owner_int != 0;
        let settings: serde_json::Value =
            serde_json::from_str(&settings_str).unwrap_or_else(|_| serde_json::json!({}));

        store
            .db()
            .query(
                "UPSERT type::thing('user', $id) CONTENT {
                    username: $username,
                    display_name: $display_name,
                    is_owner: $is_owner,
                    settings: $settings,
                    created_at: $created_at,
                    updated_at: $updated_at
                }",
            )
            .bind(("id", id.clone()))
            .bind(("username", username))
            .bind(("display_name", display_name))
            .bind(("is_owner", is_owner))
            .bind(("settings", settings))
            .bind(("created_at", created_at))
            .bind(("updated_at", updated_at))
            .await?
            .check()
            .with_context(|| format!("Failed to upsert user {}", id))?;
        count += 1;
    }
    info!("Migrated {} users", count);
    Ok(count)
}

async fn migrate_stores(sqlite: &Connection, store: &SurrealStore) -> Result<usize> {
    let mut stmt = sqlite.prepare(
        "SELECT id, owner_id, store_type, name, lancedb_collection, created_at, updated_at \
         FROM knowledge_stores",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(KnowledgeStore {
            id: row.get(0)?,
            owner_id: row.get(1)?,
            store_type: row.get(2)?,
            name: row.get(3)?,
            lancedb_collection: row.get(4)?,
            // Legacy schema has no quantizer_version column; hard-code default.
            quantizer_version: "ivf_pq_v1".to_string(),
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
        })
    })?;

    let mut count = 0usize;
    for row in rows {
        let ks = row.context("Failed to read knowledge_store row from SQLite")?;
        store
            .db()
            .query(
                "UPSERT type::thing('knowledge_store', $id) CONTENT {
                    owner_id: $owner_id,
                    store_type: $store_type,
                    name: $name,
                    lancedb_collection: $lancedb_collection,
                    quantizer_version: $quantizer_version,
                    created_at: $created_at,
                    updated_at: $updated_at
                }",
            )
            .bind(("id", ks.id.clone()))
            .bind(("owner_id", ks.owner_id))
            .bind(("store_type", ks.store_type))
            .bind(("name", ks.name))
            .bind(("lancedb_collection", ks.lancedb_collection))
            .bind(("quantizer_version", ks.quantizer_version))
            .bind(("created_at", ks.created_at))
            .bind(("updated_at", ks.updated_at))
            .await?
            .check()
            .with_context(|| format!("Failed to upsert knowledge_store {}", ks.id))?;
        count += 1;
    }
    info!("Migrated {} knowledge_stores", count);
    Ok(count)
}

async fn migrate_articles(sqlite: &Connection, store: &SurrealStore) -> Result<usize> {
    let mut stmt = sqlite.prepare(
        "SELECT id, store_id, title, content, source_type, tags, embedded_at, created_at, updated_at \
         FROM articles",
    )?;

    let rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let store_id: String = row.get(1)?;
        let title: String = row.get(2)?;
        let content: String = row.get(3)?;
        let source_type: String = row.get(4)?;
        let tags_str: String = row.get(5)?;
        let embedded_at: Option<String> = row.get(6)?;
        let created_at: String = row.get(7)?;
        let updated_at: String = row.get(8)?;
        Ok((id, store_id, title, content, source_type, tags_str, embedded_at, created_at, updated_at))
    })?;

    let mut count = 0usize;
    for row in rows {
        let (id, store_id, title, content, source_type, tags_str, embedded_at, created_at, updated_at) =
            row.context("Failed to read article row from SQLite")?;

        // Legacy has no source_id; set empty. Compute content_hash from content.
        let content_hash = hash::content_hash(&content);
        let tags: serde_json::Value =
            serde_json::from_str(&tags_str).unwrap_or_else(|_| serde_json::json!([]));

        store
            .db()
            .query(
                "UPSERT type::thing('article', $id) CONTENT {
                    store_id: $store_id,
                    title: $title,
                    content: $content,
                    source_type: $source_type,
                    source_id: $source_id,
                    content_hash: $content_hash,
                    tags: $tags,
                    embedded_at: $embedded_at,
                    created_at: $created_at,
                    updated_at: $updated_at
                }",
            )
            .bind(("id", id.clone()))
            .bind(("store_id", store_id))
            .bind(("title", title))
            .bind(("content", content))
            .bind(("source_type", source_type))
            .bind(("source_id", String::new()))
            .bind(("content_hash", content_hash))
            .bind(("tags", tags))
            .bind(("embedded_at", embedded_at))
            .bind(("created_at", created_at))
            .bind(("updated_at", updated_at))
            .await?
            .check()
            .with_context(|| format!("Failed to upsert article {}", id))?;
        count += 1;
    }
    info!("Migrated {} articles", count);
    Ok(count)
}

async fn migrate_conversations(sqlite: &Connection, store: &SurrealStore) -> Result<usize> {
    let mut stmt = sqlite.prepare(
        "SELECT id, user_id, title, message_count, created_at, updated_at FROM conversations",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(Conversation {
            id: row.get(0)?,
            user_id: row.get(1)?,
            title: row.get(2)?,
            message_count: row.get(3)?,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
        })
    })?;

    let mut count = 0usize;
    for row in rows {
        let c = row.context("Failed to read conversation row from SQLite")?;
        store
            .db()
            .query(
                "UPSERT type::thing('conversation', $id) CONTENT {
                    user_id: $user_id,
                    title: $title,
                    message_count: $message_count,
                    created_at: $created_at,
                    updated_at: $updated_at
                }",
            )
            .bind(("id", c.id.clone()))
            .bind(("user_id", c.user_id))
            .bind(("title", c.title))
            .bind(("message_count", c.message_count))
            .bind(("created_at", c.created_at))
            .bind(("updated_at", c.updated_at))
            .await?
            .check()
            .with_context(|| format!("Failed to upsert conversation {}", c.id))?;
        count += 1;
    }
    info!("Migrated {} conversations", count);
    Ok(count)
}

async fn migrate_messages(sqlite: &Connection, store: &SurrealStore) -> Result<usize> {
    let mut stmt = sqlite.prepare(
        "SELECT id, conversation_id, role, content, metadata, created_at FROM messages",
    )?;

    let rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let conversation_id: String = row.get(1)?;
        let role: String = row.get(2)?;
        let content: String = row.get(3)?;
        let metadata_str: String = row.get(4)?;
        let created_at: String = row.get(5)?;
        Ok((id, conversation_id, role, content, metadata_str, created_at))
    })?;

    let mut count = 0usize;
    for row in rows {
        let (id, conversation_id, role, content, metadata_str, created_at) =
            row.context("Failed to read message row from SQLite")?;
        let metadata: serde_json::Value =
            serde_json::from_str(&metadata_str).unwrap_or_else(|_| serde_json::json!({}));

        // Use direct UPSERT, NOT create_message which bumps message_count.
        // The legacy conversation.message_count is authoritative.
        store
            .db()
            .query(
                "UPSERT type::thing('message', $id) CONTENT {
                    conversation_id: $conversation_id,
                    role: $role,
                    content: $content,
                    metadata: $metadata,
                    created_at: $created_at
                }",
            )
            .bind(("id", id.clone()))
            .bind(("conversation_id", conversation_id))
            .bind(("role", role))
            .bind(("content", content))
            .bind(("metadata", metadata))
            .bind(("created_at", created_at))
            .await?
            .check()
            .with_context(|| format!("Failed to upsert message {}", id))?;
        count += 1;
    }
    info!("Migrated {} messages", count);
    Ok(count)
}

async fn migrate_k2k_clients(sqlite: &Connection, store: &SurrealStore) -> Result<usize> {
    let mut stmt = sqlite.prepare(
        "SELECT client_id, public_key_pem, client_name, registered_at, status FROM k2k_clients",
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

    let mut count = 0usize;
    for row in rows {
        let client = row.context("Failed to read k2k_client row from SQLite")?;
        // upsert_k2k_client already uses UPSERT internally.
        store
            .upsert_k2k_client(&client)
            .await
            .with_context(|| format!("Failed to upsert k2k_client {}", client.client_id))?;
        count += 1;
    }
    info!("Migrated {} k2k_clients", count);
    Ok(count)
}

async fn migrate_federation_agreements(
    sqlite: &Connection,
    store: &SurrealStore,
) -> Result<usize> {
    let mut stmt = sqlite.prepare(
        "SELECT id, local_store_id, remote_node_id, remote_endpoint, access_type, created_at \
         FROM federation_agreements",
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

    let mut count = 0usize;
    for row in rows {
        let fa = row.context("Failed to read federation_agreement row from SQLite")?;
        store
            .db()
            .query(
                "UPSERT type::thing('federation_agreement', $id) CONTENT {
                    local_store_id: $local_store_id,
                    remote_node_id: $remote_node_id,
                    remote_endpoint: $remote_endpoint,
                    access_type: $access_type,
                    created_at: $created_at
                }",
            )
            .bind(("id", fa.id.clone()))
            .bind(("local_store_id", fa.local_store_id))
            .bind(("remote_node_id", fa.remote_node_id))
            .bind(("remote_endpoint", fa.remote_endpoint))
            .bind(("access_type", fa.access_type))
            .bind(("created_at", fa.created_at))
            .await?
            .check()
            .with_context(|| format!("Failed to upsert federation_agreement {}", fa.id))?;
        count += 1;
    }
    info!("Migrated {} federation_agreements", count);
    Ok(count)
}

async fn migrate_discovered_nodes(sqlite: &Connection, store: &SurrealStore) -> Result<usize> {
    let mut stmt = sqlite.prepare(
        "SELECT node_id, host, port, endpoint, capabilities, last_seen, healthy \
         FROM discovered_nodes",
    )?;

    let rows = stmt.query_map([], |row| {
        let node_id: String = row.get(0)?;
        let host: String = row.get(1)?;
        let port_int: i64 = row.get(2)?;
        let endpoint: String = row.get(3)?;
        let capabilities_str: String = row.get(4)?;
        let last_seen: String = row.get(5)?;
        let healthy_int: i64 = row.get(6)?;
        Ok((node_id, host, port_int, endpoint, capabilities_str, last_seen, healthy_int))
    })?;

    let mut count = 0usize;
    for row in rows {
        let (node_id, host, port_int, endpoint, capabilities_str, last_seen, healthy_int) =
            row.context("Failed to read discovered_node row from SQLite")?;

        let port = u16::try_from(port_int)
            .with_context(|| format!("Port {} out of u16 range for node {}", port_int, node_id))?;
        let healthy = healthy_int != 0;
        let capabilities: serde_json::Value =
            serde_json::from_str(&capabilities_str).unwrap_or_else(|_| serde_json::json!([]));

        let node = DiscoveredNode {
            node_id: node_id.clone(),
            host,
            port,
            endpoint,
            capabilities,
            last_seen,
            healthy,
        };
        // upsert_discovered_node already uses UPSERT internally.
        store
            .upsert_discovered_node(&node)
            .await
            .with_context(|| format!("Failed to upsert discovered_node {}", node_id))?;
        count += 1;
    }
    info!("Migrated {} discovered_nodes", count);
    Ok(count)
}

async fn migrate_connector_configs(sqlite: &Connection, store: &SurrealStore) -> Result<usize> {
    let mut stmt = sqlite.prepare(
        "SELECT id, connector_type, name, config, store_id, created_at, updated_at \
         FROM connector_configs",
    )?;

    let rows = stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let connector_type: String = row.get(1)?;
        let name: String = row.get(2)?;
        let config_str: String = row.get(3)?;
        let store_id: String = row.get(4)?;
        let created_at: String = row.get(5)?;
        let updated_at: String = row.get(6)?;
        Ok((id, connector_type, name, config_str, store_id, created_at, updated_at))
    })?;

    let mut count = 0usize;
    for row in rows {
        let (id, connector_type, name, config_str, store_id, created_at, updated_at) =
            row.context("Failed to read connector_config row from SQLite")?;
        let config: serde_json::Value =
            serde_json::from_str(&config_str).unwrap_or_else(|_| serde_json::json!({}));

        store
            .db()
            .query(
                "UPSERT type::thing('connector_config', $id) CONTENT {
                    connector_type: $connector_type,
                    name: $name,
                    config: $config,
                    store_id: $store_id,
                    created_at: $created_at,
                    updated_at: $updated_at
                }",
            )
            .bind(("id", id.clone()))
            .bind(("connector_type", connector_type))
            .bind(("name", name))
            .bind(("config", config))
            .bind(("store_id", store_id))
            .bind(("created_at", created_at))
            .bind(("updated_at", updated_at))
            .await?
            .check()
            .with_context(|| format!("Failed to upsert connector_config {}", id))?;
        count += 1;
    }
    info!("Migrated {} connector_configs", count);
    Ok(count)
}
