//! Repository layer backed by SurrealDB.
//!
//! The `Store` trait abstracts over the concrete backend so tests and future
//! phases (e.g. P3 graph writes, a hypothetical mock backend for the router)
//! can swap in fakes. In 1.0.0 there is exactly one impl: `SurrealStore`.

pub mod hash;
pub mod migrations;
pub mod models;
pub mod schema;

pub use models::*;

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use surrealdb::engine::any::{connect, Any};
use surrealdb::Surreal;

/// Repository trait for the knowledge-nexus-local relational layer.
///
/// All methods are async because the underlying SurrealDB client is async.
/// Callers already run inside tokio (axum handlers, background tasks);
/// the few synchronous call sites (e.g. `router::planner`) are hoisted
/// into async contexts during P1 rewiring.
#[async_trait]
pub trait Store: Send + Sync {
    // Users
    async fn create_user(&self, user: &User) -> Result<()>;
    async fn get_user(&self, id: &str) -> Result<Option<User>>;
    async fn get_owner_user(&self) -> Result<Option<User>>;
    async fn list_users(&self) -> Result<Vec<User>>;

    // Knowledge stores
    async fn create_store(&self, store: &KnowledgeStore) -> Result<()>;
    async fn get_store(&self, id: &str) -> Result<Option<KnowledgeStore>>;
    async fn list_stores(&self) -> Result<Vec<KnowledgeStore>>;
    async fn list_stores_for_user(&self, owner_id: &str) -> Result<Vec<KnowledgeStore>>;

    // Articles
    async fn create_article(&self, article: &Article) -> Result<()>;
    async fn get_article(&self, id: &str) -> Result<Option<Article>>;
    async fn update_article(&self, article: &Article) -> Result<()>;
    async fn delete_article(&self, id: &str) -> Result<()>;
    async fn list_articles_for_store(&self, store_id: &str) -> Result<Vec<Article>>;
    async fn count_articles_for_owner(&self, owner_id: &str) -> Result<usize>;

    // Conversations + messages
    async fn create_conversation(&self, conv: &Conversation) -> Result<()>;
    async fn get_conversation(&self, id: &str) -> Result<Option<Conversation>>;
    async fn list_conversations_for_user(&self, user_id: &str) -> Result<Vec<Conversation>>;
    async fn create_message(&self, msg: &Message) -> Result<()>;
    async fn list_messages_for_conversation(&self, conversation_id: &str) -> Result<Vec<Message>>;

    // K2K clients
    async fn upsert_k2k_client(&self, client: &K2KClient) -> Result<()>;
    async fn get_k2k_client(&self, client_id: &str) -> Result<Option<K2KClient>>;
    async fn list_k2k_clients(&self) -> Result<Vec<K2KClient>>;
    async fn list_pending_k2k_clients(&self) -> Result<Vec<K2KClient>>;
    async fn update_k2k_client_status(&self, client_id: &str, status: &str) -> Result<()>;
    async fn delete_k2k_client(&self, client_id: &str) -> Result<()>;

    // Federation agreements
    async fn create_federation_agreement(&self, agreement: &FederationAgreement) -> Result<()>;
    async fn list_federation_agreements(&self) -> Result<Vec<FederationAgreement>>;
    async fn delete_federation_agreement(&self, id: &str) -> Result<()>;

    // Discovered nodes
    async fn upsert_discovered_node(&self, node: &DiscoveredNode) -> Result<()>;
    async fn list_discovered_nodes(&self) -> Result<Vec<DiscoveredNode>>;
    async fn mark_node_unhealthy(&self, node_id: &str) -> Result<()>;
    async fn delete_discovered_node(&self, node_id: &str) -> Result<()>;

    // Connector configs
    async fn create_connector_config(&self, config: &ConnectorConfig) -> Result<()>;
    async fn list_connector_configs(&self) -> Result<Vec<ConnectorConfig>>;
    async fn delete_connector_config(&self, id: &str) -> Result<()>;

    // Full-text search (replaces SQLite FTS5)
    async fn fts_search_articles(&self, query: &str, limit: usize) -> Result<Vec<Article>>;

    // Article-hash lookup (new in 1.0.0; wired by P3 dedup but stub lives here).
    async fn find_article_by_hash(
        &self,
        store_id: &str,
        content_hash: &str,
    ) -> Result<Option<Article>>;
}

const SURREAL_NS: &str = "knowledge_nexus";
const SURREAL_DB: &str = "local";

/// Concrete `Store` impl backed by an embedded SurrealDB.
pub struct SurrealStore {
    db: Arc<Surreal<Any>>,
}

impl SurrealStore {
    /// Open an on-disk SurrealDB at `path` using the pure-Rust `kv-surrealkv`
    /// backend. Creates the directory if it does not exist, applies DDL, and
    /// records the schema version.
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::create_dir_all(path)?;

        let endpoint = format!("surrealkv://{}", path.display());
        let db = connect(endpoint.as_str())
            .await
            .with_context(|| format!("Failed to open SurrealDB at {:?}", path))?;
        db.use_ns(SURREAL_NS).use_db(SURREAL_DB).await?;
        migrations::run_migrations(&db).await?;

        // Restrict DB directory to owner-only so other local users cannot
        // read indexed content. Applied after open so the file handles exist.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
                .with_context(|| format!("Failed to chmod {:?} to 0o700", path))?;
        }

        tracing::info!("SurrealDB opened at {:?}", path);
        Ok(Self { db: Arc::new(db) })
    }

    /// In-memory store for tests.
    pub async fn open_in_memory() -> Result<Self> {
        let db = connect("memory").await?;
        db.use_ns(SURREAL_NS).use_db(SURREAL_DB).await?;
        migrations::run_migrations(&db).await?;
        Ok(Self { db: Arc::new(db) })
    }

    fn db(&self) -> &Surreal<Any> {
        &self.db
    }
}

#[async_trait]
impl Store for SurrealStore {
    // Users
    async fn create_user(&self, user: &User) -> Result<()> {
        self.db()
            .query(
                "CREATE type::thing('user', $id) CONTENT {
                    username: $username,
                    display_name: $display_name,
                    is_owner: $is_owner,
                    settings: $settings,
                    created_at: $created_at,
                    updated_at: $updated_at
                }",
            )
            .bind(("id", user.id.clone()))
            .bind(("username", user.username.clone()))
            .bind(("display_name", user.display_name.clone()))
            .bind(("is_owner", user.is_owner))
            .bind(("settings", user.settings.clone()))
            .bind(("created_at", user.created_at.clone()))
            .bind(("updated_at", user.updated_at.clone()))
            .await?
            .check()?;
        Ok(())
    }

    async fn get_user(&self, id: &str) -> Result<Option<User>> {
        let mut resp = self
            .db()
            .query("SELECT *, meta::id(id) AS id FROM type::thing('user', $id)")
            .bind(("id", id.to_string()))
            .await?;
        let users: Vec<User> = resp.take(0)?;
        Ok(users.into_iter().next())
    }

    async fn get_owner_user(&self) -> Result<Option<User>> {
        let mut resp = self
            .db()
            .query(
                "SELECT *, meta::id(id) AS id FROM user WHERE is_owner = true LIMIT 1",
            )
            .await?;
        let users: Vec<User> = resp.take(0)?;
        Ok(users.into_iter().next())
    }

    async fn list_users(&self) -> Result<Vec<User>> {
        let mut resp = self
            .db()
            .query("SELECT *, meta::id(id) AS id FROM user ORDER BY created_at")
            .await?;
        let users: Vec<User> = resp.take(0)?;
        Ok(users)
    }

    // KnowledgeStore CRUD
    async fn create_store(&self, s: &KnowledgeStore) -> Result<()> {
        self.db()
            .query(
                "CREATE type::thing('knowledge_store', $id) CONTENT {
                    owner_id: $owner_id,
                    store_type: $store_type,
                    name: $name,
                    lancedb_collection: $lancedb_collection,
                    quantizer_version: $quantizer_version,
                    created_at: $created_at,
                    updated_at: $updated_at
                }",
            )
            .bind(("id", s.id.clone()))
            .bind(("owner_id", s.owner_id.clone()))
            .bind(("store_type", s.store_type.clone()))
            .bind(("name", s.name.clone()))
            .bind(("lancedb_collection", s.lancedb_collection.clone()))
            .bind(("quantizer_version", s.quantizer_version.clone()))
            .bind(("created_at", s.created_at.clone()))
            .bind(("updated_at", s.updated_at.clone()))
            .await?
            .check()?;
        Ok(())
    }

    async fn get_store(&self, id: &str) -> Result<Option<KnowledgeStore>> {
        let mut resp = self
            .db()
            .query("SELECT *, meta::id(id) AS id FROM type::thing('knowledge_store', $id)")
            .bind(("id", id.to_string()))
            .await?;
        let rows: Vec<KnowledgeStore> = resp.take(0)?;
        Ok(rows.into_iter().next())
    }

    async fn list_stores(&self) -> Result<Vec<KnowledgeStore>> {
        let mut resp = self
            .db()
            .query("SELECT *, meta::id(id) AS id FROM knowledge_store ORDER BY created_at")
            .await?;
        Ok(resp.take(0)?)
    }

    async fn list_stores_for_user(&self, owner_id: &str) -> Result<Vec<KnowledgeStore>> {
        let mut resp = self
            .db()
            .query(
                "SELECT *, meta::id(id) AS id FROM knowledge_store
                 WHERE owner_id = $owner_id ORDER BY created_at",
            )
            .bind(("owner_id", owner_id.to_string()))
            .await?;
        Ok(resp.take(0)?)
    }

    // Article CRUD
    async fn create_article(&self, a: &Article) -> Result<()> {
        self.db()
            .query(
                "CREATE type::thing('article', $id) CONTENT {
                    store_id: $store_id, title: $title, content: $content,
                    source_type: $source_type, source_id: $source_id,
                    content_hash: $content_hash, tags: $tags,
                    embedded_at: $embedded_at, created_at: $created_at,
                    updated_at: $updated_at
                }",
            )
            .bind(("id", a.id.clone()))
            .bind(("store_id", a.store_id.clone()))
            .bind(("title", a.title.clone()))
            .bind(("content", a.content.clone()))
            .bind(("source_type", a.source_type.clone()))
            .bind(("source_id", a.source_id.clone()))
            .bind(("content_hash", a.content_hash.clone()))
            .bind(("tags", a.tags.clone()))
            .bind(("embedded_at", a.embedded_at.clone()))
            .bind(("created_at", a.created_at.clone()))
            .bind(("updated_at", a.updated_at.clone()))
            .await?
            .check()?;
        Ok(())
    }

    async fn get_article(&self, id: &str) -> Result<Option<Article>> {
        let mut resp = self
            .db()
            .query("SELECT *, meta::id(id) AS id FROM type::thing('article', $id)")
            .bind(("id", id.to_string()))
            .await?;
        let rows: Vec<Article> = resp.take(0)?;
        Ok(rows.into_iter().next())
    }

    async fn update_article(&self, a: &Article) -> Result<()> {
        self.db()
            .query(
                "UPDATE type::thing('article', $id) MERGE {
                    title: $title, content: $content,
                    source_type: $source_type, source_id: $source_id,
                    content_hash: $content_hash, tags: $tags,
                    embedded_at: $embedded_at, updated_at: $updated_at
                }",
            )
            .bind(("id", a.id.clone()))
            .bind(("title", a.title.clone()))
            .bind(("content", a.content.clone()))
            .bind(("source_type", a.source_type.clone()))
            .bind(("source_id", a.source_id.clone()))
            .bind(("content_hash", a.content_hash.clone()))
            .bind(("tags", a.tags.clone()))
            .bind(("embedded_at", a.embedded_at.clone()))
            .bind(("updated_at", a.updated_at.clone()))
            .await?
            .check()?;
        Ok(())
    }

    async fn delete_article(&self, id: &str) -> Result<()> {
        self.db()
            .query("DELETE type::thing('article', $id)")
            .bind(("id", id.to_string()))
            .await?
            .check()?;
        Ok(())
    }

    async fn list_articles_for_store(&self, store_id: &str) -> Result<Vec<Article>> {
        let mut resp = self
            .db()
            .query(
                "SELECT *, meta::id(id) AS id FROM article
                 WHERE store_id = $store_id ORDER BY created_at DESC",
            )
            .bind(("store_id", store_id.to_string()))
            .await?;
        Ok(resp.take(0)?)
    }

    async fn count_articles_for_owner(&self, owner_id: &str) -> Result<usize> {
        let mut resp = self
            .db()
            .query(
                "LET $store_ids = (SELECT VALUE meta::id(id) FROM knowledge_store WHERE owner_id = $owner_id);
                 RETURN count(SELECT * FROM article WHERE store_id IN $store_ids);",
            )
            .bind(("owner_id", owner_id.to_string()))
            .await?;
        let count: Option<i64> = resp.take(1)?;
        Ok(count.unwrap_or(0) as usize)
    }

    async fn find_article_by_hash(&self, store_id: &str, content_hash: &str) -> Result<Option<Article>> {
        let mut resp = self
            .db()
            .query(
                "SELECT *, meta::id(id) AS id FROM article
                 WHERE store_id = $store_id AND content_hash = $hash LIMIT 1",
            )
            .bind(("store_id", store_id.to_string()))
            .bind(("hash", content_hash.to_string()))
            .await?;
        let rows: Vec<Article> = resp.take(0)?;
        Ok(rows.into_iter().next())
    }

    // Conversation + Message CRUD
    async fn create_conversation(&self, c: &Conversation) -> Result<()> {
        self.db()
            .query(
                "CREATE type::thing('conversation', $id) CONTENT {
                    user_id: $user_id, title: $title,
                    message_count: $message_count,
                    created_at: $created_at, updated_at: $updated_at
                }",
            )
            .bind(("id", c.id.clone()))
            .bind(("user_id", c.user_id.clone()))
            .bind(("title", c.title.clone()))
            .bind(("message_count", c.message_count))
            .bind(("created_at", c.created_at.clone()))
            .bind(("updated_at", c.updated_at.clone()))
            .await?
            .check()?;
        Ok(())
    }

    async fn get_conversation(&self, id: &str) -> Result<Option<Conversation>> {
        let mut resp = self
            .db()
            .query("SELECT *, meta::id(id) AS id FROM type::thing('conversation', $id)")
            .bind(("id", id.to_string()))
            .await?;
        let rows: Vec<Conversation> = resp.take(0)?;
        Ok(rows.into_iter().next())
    }

    async fn list_conversations_for_user(&self, user_id: &str) -> Result<Vec<Conversation>> {
        let mut resp = self
            .db()
            .query(
                "SELECT *, meta::id(id) AS id FROM conversation
                 WHERE user_id = $user_id ORDER BY updated_at DESC",
            )
            .bind(("user_id", user_id.to_string()))
            .await?;
        Ok(resp.take(0)?)
    }

    async fn create_message(&self, m: &Message) -> Result<()> {
        self.db()
            .query(
                "BEGIN TRANSACTION;
                 CREATE type::thing('message', $id) CONTENT {
                    conversation_id: $conversation_id,
                    role: $role, content: $content,
                    metadata: $metadata, created_at: $created_at
                 };
                 UPDATE type::thing('conversation', $conversation_id)
                    SET message_count = message_count + 1, updated_at = $created_at;
                 COMMIT TRANSACTION;",
            )
            .bind(("id", m.id.clone()))
            .bind(("conversation_id", m.conversation_id.clone()))
            .bind(("role", m.role.clone()))
            .bind(("content", m.content.clone()))
            .bind(("metadata", m.metadata.clone()))
            .bind(("created_at", m.created_at.clone()))
            .await?
            .check()?;
        Ok(())
    }

    async fn list_messages_for_conversation(&self, conversation_id: &str) -> Result<Vec<Message>> {
        let mut resp = self
            .db()
            .query(
                "SELECT *, meta::id(id) AS id FROM message
                 WHERE conversation_id = $conversation_id ORDER BY created_at ASC",
            )
            .bind(("conversation_id", conversation_id.to_string()))
            .await?;
        Ok(resp.take(0)?)
    }

    // K2KClient CRUD
    async fn upsert_k2k_client(&self, c: &K2KClient) -> Result<()> {
        self.db()
            .query(
                "UPDATE type::thing('k2k_client', $id) CONTENT {
                    public_key_pem: $public_key_pem,
                    client_name: $client_name,
                    registered_at: $registered_at,
                    status: $status
                 }",
            )
            .bind(("id", c.client_id.clone()))
            .bind(("public_key_pem", c.public_key_pem.clone()))
            .bind(("client_name", c.client_name.clone()))
            .bind(("registered_at", c.registered_at.clone()))
            .bind(("status", c.status.clone()))
            .await?
            .check()?;
        Ok(())
    }

    async fn get_k2k_client(&self, client_id: &str) -> Result<Option<K2KClient>> {
        let mut resp = self
            .db()
            .query("SELECT *, meta::id(id) AS client_id FROM type::thing('k2k_client', $id)")
            .bind(("id", client_id.to_string()))
            .await?;
        let rows: Vec<K2KClient> = resp.take(0)?;
        Ok(rows.into_iter().next())
    }

    async fn list_k2k_clients(&self) -> Result<Vec<K2KClient>> {
        let mut resp = self
            .db()
            .query("SELECT *, meta::id(id) AS client_id FROM k2k_client ORDER BY registered_at")
            .await?;
        Ok(resp.take(0)?)
    }

    async fn list_pending_k2k_clients(&self) -> Result<Vec<K2KClient>> {
        let mut resp = self
            .db()
            .query(
                "SELECT *, meta::id(id) AS client_id FROM k2k_client
                 WHERE status = 'pending' ORDER BY registered_at",
            )
            .await?;
        Ok(resp.take(0)?)
    }

    async fn update_k2k_client_status(&self, client_id: &str, status: &str) -> Result<()> {
        self.db()
            .query("UPDATE type::thing('k2k_client', $id) SET status = $status")
            .bind(("id", client_id.to_string()))
            .bind(("status", status.to_string()))
            .await?
            .check()?;
        Ok(())
    }

    async fn delete_k2k_client(&self, client_id: &str) -> Result<()> {
        self.db()
            .query("DELETE type::thing('k2k_client', $id)")
            .bind(("id", client_id.to_string()))
            .await?
            .check()?;
        Ok(())
    }

    // Federation + DiscoveredNode + ConnectorConfig CRUD
    async fn create_federation_agreement(&self, a: &FederationAgreement) -> Result<()> {
        self.db()
            .query(
                "CREATE type::thing('federation_agreement', $id) CONTENT {
                    local_store_id: $local_store_id,
                    remote_node_id: $remote_node_id,
                    remote_endpoint: $remote_endpoint,
                    access_type: $access_type,
                    created_at: $created_at
                 }",
            )
            .bind(("id", a.id.clone()))
            .bind(("local_store_id", a.local_store_id.clone()))
            .bind(("remote_node_id", a.remote_node_id.clone()))
            .bind(("remote_endpoint", a.remote_endpoint.clone()))
            .bind(("access_type", a.access_type.clone()))
            .bind(("created_at", a.created_at.clone()))
            .await?
            .check()?;
        Ok(())
    }

    async fn list_federation_agreements(&self) -> Result<Vec<FederationAgreement>> {
        let mut resp = self
            .db()
            .query("SELECT *, meta::id(id) AS id FROM federation_agreement ORDER BY created_at")
            .await?;
        Ok(resp.take(0)?)
    }

    async fn delete_federation_agreement(&self, id: &str) -> Result<()> {
        self.db()
            .query("DELETE type::thing('federation_agreement', $id)")
            .bind(("id", id.to_string()))
            .await?
            .check()?;
        Ok(())
    }

    async fn upsert_discovered_node(&self, n: &DiscoveredNode) -> Result<()> {
        self.db()
            .query(
                "UPDATE type::thing('discovered_node', $id) CONTENT {
                    host: $host, port: $port, endpoint: $endpoint,
                    capabilities: $capabilities,
                    last_seen: $last_seen, healthy: $healthy
                 }",
            )
            .bind(("id", n.node_id.clone()))
            .bind(("host", n.host.clone()))
            .bind(("port", n.port as i64))
            .bind(("endpoint", n.endpoint.clone()))
            .bind(("capabilities", n.capabilities.clone()))
            .bind(("last_seen", n.last_seen.clone()))
            .bind(("healthy", n.healthy))
            .await?
            .check()?;
        Ok(())
    }

    async fn list_discovered_nodes(&self) -> Result<Vec<DiscoveredNode>> {
        let mut resp = self
            .db()
            .query("SELECT *, meta::id(id) AS node_id FROM discovered_node ORDER BY last_seen DESC")
            .await?;
        Ok(resp.take(0)?)
    }

    async fn mark_node_unhealthy(&self, node_id: &str) -> Result<()> {
        self.db()
            .query("UPDATE type::thing('discovered_node', $id) SET healthy = false")
            .bind(("id", node_id.to_string()))
            .await?
            .check()?;
        Ok(())
    }

    async fn delete_discovered_node(&self, node_id: &str) -> Result<()> {
        self.db()
            .query("DELETE type::thing('discovered_node', $id)")
            .bind(("id", node_id.to_string()))
            .await?
            .check()?;
        Ok(())
    }

    async fn create_connector_config(&self, c: &ConnectorConfig) -> Result<()> {
        self.db()
            .query(
                "CREATE type::thing('connector_config', $id) CONTENT {
                    connector_type: $connector_type,
                    name: $name, config: $config,
                    store_id: $store_id,
                    created_at: $created_at, updated_at: $updated_at
                 }",
            )
            .bind(("id", c.id.clone()))
            .bind(("connector_type", c.connector_type.clone()))
            .bind(("name", c.name.clone()))
            .bind(("config", c.config.clone()))
            .bind(("store_id", c.store_id.clone()))
            .bind(("created_at", c.created_at.clone()))
            .bind(("updated_at", c.updated_at.clone()))
            .await?
            .check()?;
        Ok(())
    }

    async fn list_connector_configs(&self) -> Result<Vec<ConnectorConfig>> {
        let mut resp = self
            .db()
            .query("SELECT *, meta::id(id) AS id FROM connector_config ORDER BY created_at")
            .await?;
        Ok(resp.take(0)?)
    }

    async fn delete_connector_config(&self, id: &str) -> Result<()> {
        self.db()
            .query("DELETE type::thing('connector_config', $id)")
            .bind(("id", id.to_string()))
            .await?
            .check()?;
        Ok(())
    }

    async fn fts_search_articles(&self, _q: &str, _l: usize) -> Result<Vec<Article>> { unimplemented!() }
}

#[cfg(test)]
mod store_tests {
    use super::*;

    fn now() -> String { chrono::Utc::now().to_rfc3339() }

    async fn fixture() -> SurrealStore {
        let s = SurrealStore::open_in_memory().await.unwrap();
        let ts = now();
        s.create_user(&User {
            id: "u1".into(), username: "alice".into(), display_name: "Alice".into(),
            is_owner: true, settings: serde_json::json!({}),
            created_at: ts.clone(), updated_at: ts,
        }).await.unwrap();
        s
    }

    #[tokio::test]
    async fn test_store_crud() {
        let s = fixture().await;
        let ts = now();
        s.create_store(&KnowledgeStore {
            id: "s1".into(), owner_id: "u1".into(), store_type: "personal".into(),
            name: "Alice's Notes".into(), lancedb_collection: "store_s1".into(),
            quantizer_version: "ivf_pq_v1".into(),
            created_at: ts.clone(), updated_at: ts,
        }).await.unwrap();

        let got = s.get_store("s1").await.unwrap().expect("exists");
        assert_eq!(got.name, "Alice's Notes");
        assert_eq!(got.quantizer_version, "ivf_pq_v1");

        assert_eq!(s.list_stores().await.unwrap().len(), 1);
        assert_eq!(s.list_stores_for_user("u1").await.unwrap().len(), 1);
        assert_eq!(s.list_stores_for_user("u2").await.unwrap().len(), 0);
    }
}

#[cfg(test)]
mod article_tests {
    use super::*;

    fn now() -> String { chrono::Utc::now().to_rfc3339() }

    async fn fixture() -> SurrealStore {
        let s = SurrealStore::open_in_memory().await.unwrap();
        let ts = now();
        s.create_user(&User {
            id: "u1".into(), username: "alice".into(), display_name: "Alice".into(),
            is_owner: true, settings: serde_json::json!({}),
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_store(&KnowledgeStore {
            id: "s1".into(), owner_id: "u1".into(), store_type: "personal".into(),
            name: "Notes".into(), lancedb_collection: "store_s1".into(),
            quantizer_version: "ivf_pq_v1".into(),
            created_at: ts.clone(), updated_at: ts,
        }).await.unwrap();
        s
    }

    #[tokio::test]
    async fn test_article_crud_and_hash() {
        let s = fixture().await;
        let ts = now();
        let article = Article {
            id: "a1".into(), store_id: "s1".into(), title: "Test".into(),
            content: "Hello world".into(), source_type: "user".into(),
            source_id: "".into(), content_hash: "abc123".into(),
            tags: serde_json::json!(["test"]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        };
        s.create_article(&article).await.unwrap();

        let got = s.get_article("a1").await.unwrap().expect("exists");
        assert_eq!(got.title, "Test");
        assert_eq!(got.content_hash, "abc123");

        let hash_hit = s.find_article_by_hash("s1", "abc123").await.unwrap().expect("hash match");
        assert_eq!(hash_hit.id, "a1");
        assert!(s.find_article_by_hash("s1", "nope").await.unwrap().is_none());

        let mut updated = got.clone();
        updated.title = "Updated".into();
        s.update_article(&updated).await.unwrap();
        assert_eq!(s.get_article("a1").await.unwrap().unwrap().title, "Updated");

        assert_eq!(s.list_articles_for_store("s1").await.unwrap().len(), 1);
        assert_eq!(s.count_articles_for_owner("u1").await.unwrap(), 1);

        s.delete_article("a1").await.unwrap();
        assert!(s.get_article("a1").await.unwrap().is_none());
    }
}

#[cfg(test)]
mod conv_tests {
    use super::*;

    fn now() -> String { chrono::Utc::now().to_rfc3339() }

    #[tokio::test]
    async fn test_conversation_and_messages() {
        let s = SurrealStore::open_in_memory().await.unwrap();
        let ts = now();
        s.create_user(&User {
            id: "u1".into(), username: "alice".into(), display_name: "Alice".into(),
            is_owner: true, settings: serde_json::json!({}),
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        s.create_conversation(&Conversation {
            id: "c1".into(), user_id: "u1".into(), title: "Chat".into(),
            message_count: 0, created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        s.create_message(&Message {
            id: "m1".into(), conversation_id: "c1".into(),
            role: "user".into(), content: "Hello".into(),
            metadata: serde_json::json!({}), created_at: ts,
        }).await.unwrap();

        let conv = s.get_conversation("c1").await.unwrap().expect("exists");
        assert_eq!(conv.message_count, 1);

        let msgs = s.list_messages_for_conversation("c1").await.unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "Hello");

        assert_eq!(s.list_conversations_for_user("u1").await.unwrap().len(), 1);
    }
}

#[cfg(test)]
mod k2k_tests {
    use super::*;

    #[tokio::test]
    async fn test_k2k_client_crud() {
        let s = SurrealStore::open_in_memory().await.unwrap();
        let client = K2KClient {
            client_id: "client1".into(),
            public_key_pem: "-----BEGIN PUBLIC KEY-----\ntest\n-----END PUBLIC KEY-----".into(),
            client_name: "Test".into(),
            registered_at: chrono::Utc::now().to_rfc3339(),
            status: "approved".into(),
        };
        s.upsert_k2k_client(&client).await.unwrap();

        let got = s.get_k2k_client("client1").await.unwrap().expect("exists");
        assert_eq!(got.client_name, "Test");
        assert_eq!(s.list_k2k_clients().await.unwrap().len(), 1);

        let mut updated = client.clone();
        updated.client_name = "Renamed".into();
        s.upsert_k2k_client(&updated).await.unwrap();
        assert_eq!(s.get_k2k_client("client1").await.unwrap().unwrap().client_name, "Renamed");

        s.update_k2k_client_status("client1", "pending").await.unwrap();
        assert_eq!(s.list_pending_k2k_clients().await.unwrap().len(), 1);

        s.delete_k2k_client("client1").await.unwrap();
        assert!(s.get_k2k_client("client1").await.unwrap().is_none());
    }
}

#[cfg(test)]
mod federation_tests {
    use super::*;

    fn now() -> String { chrono::Utc::now().to_rfc3339() }

    async fn fixture() -> SurrealStore {
        let s = SurrealStore::open_in_memory().await.unwrap();
        let ts = now();
        s.create_user(&User {
            id: "u1".into(), username: "alice".into(), display_name: "Alice".into(),
            is_owner: true, settings: serde_json::json!({}),
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_store(&KnowledgeStore {
            id: "s1".into(), owner_id: "u1".into(), store_type: "personal".into(),
            name: "N".into(), lancedb_collection: "store_s1".into(),
            quantizer_version: "ivf_pq_v1".into(),
            created_at: ts.clone(), updated_at: ts,
        }).await.unwrap();
        s
    }

    #[tokio::test]
    async fn test_federation_agreements() {
        let s = fixture().await;
        s.create_federation_agreement(&FederationAgreement {
            id: "fa1".into(), local_store_id: "s1".into(),
            remote_node_id: "node2".into(),
            remote_endpoint: "http://192.168.1.20:8765/k2k/v1".into(),
            access_type: "read".into(), created_at: now(),
        }).await.unwrap();
        assert_eq!(s.list_federation_agreements().await.unwrap().len(), 1);
        s.delete_federation_agreement("fa1").await.unwrap();
        assert!(s.list_federation_agreements().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_discovered_nodes() {
        let s = fixture().await;
        s.upsert_discovered_node(&DiscoveredNode {
            node_id: "node1".into(), host: "192.168.1.10".into(), port: 8765,
            endpoint: "http://192.168.1.10:8765/k2k/v1".into(),
            capabilities: serde_json::json!(["semantic_search"]),
            last_seen: now(), healthy: true,
        }).await.unwrap();
        let nodes = s.list_discovered_nodes().await.unwrap();
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].healthy);
        s.mark_node_unhealthy("node1").await.unwrap();
        assert!(!s.list_discovered_nodes().await.unwrap()[0].healthy);
        s.delete_discovered_node("node1").await.unwrap();
        assert!(s.list_discovered_nodes().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_connector_configs() {
        let s = fixture().await;
        s.create_connector_config(&ConnectorConfig {
            id: "cc1".into(), connector_type: "local_files".into(),
            name: "Docs".into(), config: serde_json::json!({"path": "/tmp/docs"}),
            store_id: "s1".into(), created_at: now(), updated_at: now(),
        }).await.unwrap();
        assert_eq!(s.list_connector_configs().await.unwrap().len(), 1);
        s.delete_connector_config("cc1").await.unwrap();
        assert!(s.list_connector_configs().await.unwrap().is_empty());
    }
}

#[cfg(test)]
mod open_tests {
    use super::*;

    #[tokio::test]
    async fn test_open_in_memory_applies_schema() {
        let store = SurrealStore::open_in_memory().await.unwrap();
        // Schema-version row should exist.
        let mut resp = store
            .db()
            .query("SELECT version FROM _schema_version")
            .await
            .unwrap();
        let versions: Vec<serde_json::Value> = resp.take(0).unwrap();
        assert_eq!(versions.len(), 1);
    }

    #[tokio::test]
    async fn test_open_on_disk_applies_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("surreal");
        let store = SurrealStore::open(&path).await.unwrap();
        let mut resp = store
            .db()
            .query("SELECT version FROM _schema_version")
            .await
            .unwrap();
        let versions: Vec<serde_json::Value> = resp.take(0).unwrap();
        assert_eq!(versions.len(), 1);
    }
}

#[cfg(test)]
mod user_tests {
    use super::*;

    fn now() -> String {
        chrono::Utc::now().to_rfc3339()
    }

    #[tokio::test]
    async fn test_user_crud() {
        let store = SurrealStore::open_in_memory().await.unwrap();
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
        store.create_user(&user).await.unwrap();

        let fetched = store.get_user("u1").await.unwrap().expect("user exists");
        assert_eq!(fetched.username, "alice");
        assert!(fetched.is_owner);

        let owner = store.get_owner_user().await.unwrap().expect("owner exists");
        assert_eq!(owner.id, "u1");

        let users = store.list_users().await.unwrap();
        assert_eq!(users.len(), 1);
    }
}
