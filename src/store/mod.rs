//! Repository layer backed by SurrealDB.
//!
//! The `Store` trait abstracts over the concrete backend so tests and future
//! phases (e.g. P3 graph writes, a hypothetical mock backend for the router)
//! can swap in fakes. In 1.0.0 there is exactly one impl: `SurrealStore`.

pub mod hash;
pub mod migrations;
pub mod models;
pub mod schema;
pub mod slugify;

pub use models::*;
pub use slugify::{entity_id, slugify};

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

    // Entities (P3)
    async fn create_entity(&self, entity: &Entity) -> Result<()>;
    async fn get_entity(&self, id: &str) -> Result<Option<Entity>>;
    async fn list_entities_for_store(&self, store_id: &str) -> Result<Vec<Entity>>;
    async fn upsert_entity(&self, entity: &Entity) -> Result<()>;

    /// Atomically increment an entity's mention_count by 1. Creates the entity
    /// if it doesn't exist (upsert semantics).
    async fn upsert_entity_and_increment(
        &self,
        entity: &Entity,
    ) -> Result<()>;

    // Tags (P3)
    async fn create_tag(&self, tag: &Tag) -> Result<()>;
    async fn list_tags_for_store(&self, store_id: &str) -> Result<Vec<Tag>>;
    async fn upsert_tag(&self, tag: &Tag) -> Result<()>;

    // Dedup queue (P3)
    async fn create_dedup_entry(&self, entry: &DedupQueueEntry) -> Result<()>;
    async fn list_pending_dedup(&self, store_id: &str) -> Result<Vec<DedupQueueEntry>>;
    async fn get_dedup_entry(&self, id: &str) -> Result<Option<DedupQueueEntry>>;
    async fn resolve_dedup_entry(&self, id: &str, status: &str) -> Result<()>;

    // Graph edges (P3)
    async fn create_mentions_edge(
        &self, article_id: &str, entity_id: &str, excerpt: &str, confidence: f64,
    ) -> Result<()>;
    async fn create_tagged_edge(&self, article_id: &str, tag_id: &str) -> Result<()>;
    async fn create_or_update_related_to_edge(
        &self, from_article_id: &str, to_article_id: &str,
        shared_entity_count: i64, strength: f64,
    ) -> Result<()>;
    async fn list_entities_for_article(&self, article_id: &str) -> Result<Vec<Entity>>;
    async fn list_articles_for_entity(&self, entity_id: &str) -> Result<Vec<Article>>;
    async fn list_tags_for_article(&self, article_id: &str) -> Result<Vec<Tag>>;
    async fn list_related_articles(&self, article_id: &str) -> Result<Vec<Article>>;
    async fn list_articles_without_mentions(&self, store_id: &str) -> Result<Vec<Article>>;

    // Graph queries (P4)
    async fn search_entities_by_name(&self, store_id: &str, terms: &[&str]) -> Result<Vec<Entity>>;
    async fn list_articles_for_entities(&self, entity_ids: &[&str]) -> Result<Vec<(Article, f64)>>;
    async fn count_entities_by_type(&self, store_id: &str) -> Result<std::collections::HashMap<String, usize>>;
    /// Returns entities that co-occur with the given entity in shared articles.
    ///
    /// The `usize` in each returned tuple is the number of distinct articles
    /// where both the given entity and the co-entity are mentioned. This is
    /// the co-occurrence article count — NOT the co-entity's global mention_count.
    /// Sorted by shared-article count descending.
    async fn list_co_mentioned_entities(&self, entity_id: &str) -> Result<Vec<(Entity, usize)>>;

    // P5 typed edges (Task 3)
    async fn create_precedes_edge(
        &self,
        store_id: &str,
        from_article_id: &str,
        to_article_id: &str,
        confidence: f64,
        method: ExtractionMethod,
    ) -> Result<()>;

    async fn create_semantically_related_edge(
        &self,
        store_id: &str,
        from_article_id: &str,
        to_article_id: &str,
        similarity: f64,
    ) -> Result<()>;

    async fn create_caused_by_edge(
        &self,
        store_id: &str,
        from_article_id: &str,
        to_article_id: &str,
        confidence: f64,
        rationale: Option<String>,
    ) -> Result<()>;

    async fn create_references_edge(
        &self,
        store_id: &str,
        from_article_id: &str,
        to_article_id: &str,
        anchor_text: Option<String>,
    ) -> Result<()>;

    async fn list_precedes_for(&self, store_id: &str, article_id: &str) -> Result<Vec<PrecedesEdge>>;
    async fn list_semantically_related_for(&self, store_id: &str, article_id: &str) -> Result<Vec<SemanticallyRelatedEdge>>;
    async fn list_caused_by_for(&self, store_id: &str, article_id: &str) -> Result<Vec<CausedByEdge>>;
    async fn list_references_for(&self, store_id: &str, article_id: &str) -> Result<Vec<ReferencesEdgeRow>>;

    /// Returns all (from_id, to_id) pairs from the entity_overlap table for
    /// a given store. Used by P5 backfills that operate over the existing
    /// entity-overlap graph.
    async fn list_entity_overlap_pairs(&self, store_id: &str) -> Result<Vec<(String, String)>>;

    /// Returns all article ids for the given store. Used by P5 backfills
    /// that walk the article corpus.
    async fn list_article_ids(&self, store_id: &str) -> Result<Vec<String>>;

    /// Returns articles connected to `article_id` via any of the enabled
    /// edge types. UNION across the enabled tables. Used by P6 spreading
    /// activation; P5 only adds the plumbing.
    ///
    /// The returned tuple is `(neighbor_article_id, edge_type_label, score)`.
    /// `edge_type_label` is the string name of the edge table; `score` is the
    /// strongest-confidence (or similarity for semantically_related) value.
    async fn list_graph_neighbors(
        &self,
        store_id: &str,
        article_id: &str,
        filter: &crate::config::EdgeTypeFilter,
    ) -> Result<Vec<(String, String, f64)>>;

    /// Returns per-edge-type counts for a store. Used by P5 `graph stats`
    /// CLI to surface multi-graph coverage.
    async fn count_edges_by_type(&self, store_id: &str) -> Result<EdgeCounts>;

    // P6 specificity weighting

    /// Returns a map of entity_id → mention count (number of articles
    /// mentioning the entity) for a given store. Used by P6 HippoRAG-style
    /// specificity weighting: rare entities get higher weight.
    async fn count_mentions_per_entity(&self, store_id: &str) -> Result<std::collections::HashMap<String, usize>>;
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

    pub(crate) fn db(&self) -> &Surreal<Any> {
        &self.db
    }

    /// Update only the `quantizer_version` field on a knowledge store.
    ///
    /// Used by the `reindex --quantizer` CLI command. Not on the `Store` trait
    /// because it is a SurrealDB-specific operation for now.
    pub async fn update_store_quantizer_version(
        &self,
        store_id: &str,
        quantizer_version: &str,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.db()
            .query(
                "UPDATE type::thing('knowledge_store', $id) SET quantizer_version = $qv, updated_at = $now",
            )
            .bind(("id", store_id.to_string()))
            .bind(("qv", quantizer_version.to_string()))
            .bind(("now", now))
            .await?
            .check()?;
        Ok(())
    }
}

/// Open the SurrealDB store from config, creating the owner user and default
/// knowledge store on first run. Refuses to start if a legacy SQLite DB exists
/// but no migration has been run.
///
/// This is the lib-visible equivalent of `open_store_or_bail` from `main.rs`.
/// Non-bin modules (e.g. `search::search_files`) call this instead of
/// duplicating the store-opening logic.
pub async fn open_from_config(cfg: &crate::config::Config) -> Result<std::sync::Arc<dyn Store>> {
    let surreal_dir = crate::config::data_dir().join("surreal");
    let sqlite_path = crate::config::sqlite_path();
    let surreal_exists = surreal_dir.exists()
        && surreal_dir
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(false);
    let migration_complete = crate::migrate::is_migrated(&surreal_dir);
    let legacy_sqlite_exists = sqlite_path.exists();

    match (surreal_exists, migration_complete, legacy_sqlite_exists) {
        (true, true, _) => {
            tracing::info!("Opening SurrealDB at {:?}", surreal_dir);
        }
        (true, false, _) => {
            anyhow::bail!(
                "SurrealDB directory {:?} exists but has no `migration_completed` marker. \
                 A previous migration was interrupted. Run: \
                 `knowledge-nexus-agent migrate --force` to retry.",
                surreal_dir
            );
        }
        (false, _, true) => {
            anyhow::bail!(
                "Legacy SQLite DB at {:?} detected, but no SurrealDB yet. Run: \
                 `knowledge-nexus-agent migrate --from sqlite --to surrealdb` to upgrade.",
                sqlite_path
            );
        }
        (false, _, false) => {
            tracing::info!("No existing database — creating fresh SurrealDB at {:?}", surreal_dir);
        }
    }

    let surreal_store = SurrealStore::open(&surreal_dir).await?;

    if surreal_store.get_owner_user().await?.is_none() {
        let now = chrono::Utc::now().to_rfc3339();
        let user_id = uuid::Uuid::new_v4().to_string();
        let store_id = uuid::Uuid::new_v4().to_string();

        let user = User {
            id: user_id.clone(),
            username: cfg.device.name.clone(),
            display_name: cfg.device.name.clone(),
            is_owner: true,
            settings: serde_json::json!({}),
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        surreal_store.create_user(&user).await?;
        tracing::info!("Created default owner user: {}", user.username);

        let ks = KnowledgeStore {
            id: store_id.clone(),
            owner_id: user_id,
            store_type: "personal".into(),
            name: format!("{}'s Knowledge", cfg.device.name),
            lancedb_collection: format!("store_{}", store_id),
            quantizer_version: "ivf_pq_v1".into(),
            created_at: now.clone(),
            updated_at: now,
        };
        surreal_store.create_store(&ks).await?;
        tracing::info!("Created default personal store: {}", ks.name);
    }

    Ok(std::sync::Arc::new(surreal_store))
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
                "UPSERT type::thing('k2k_client', $id) CONTENT {
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
                "UPSERT type::thing('discovered_node', $id) CONTENT {
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

    async fn fts_search_articles(&self, query: &str, limit: usize) -> Result<Vec<Article>> {
        let limit_i64 = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut resp = self
            .db()
            .query(
                "SELECT *, meta::id(id) AS id,
                        search::score(0) + search::score(1) AS _rank
                 FROM article
                 WHERE title @0@ $q OR content @1@ $q
                 ORDER BY _rank DESC
                 LIMIT $limit",
            )
            .bind(("q", query.to_string()))
            .bind(("limit", limit_i64))
            .await?;
        let rows: Vec<Article> = resp.take(0)?;
        Ok(rows)
    }

    // Entity CRUD (P3)
    async fn create_entity(&self, e: &Entity) -> Result<()> {
        self.db()
            .query(
                "CREATE type::thing('entity', $id) CONTENT {
                    name: $name, entity_type: $entity_type,
                    description: $description, store_id: $store_id,
                    mention_count: $mention_count,
                    created_at: $created_at, updated_at: $updated_at
                }",
            )
            .bind(("id", e.id.clone()))
            .bind(("name", e.name.clone()))
            .bind(("entity_type", e.entity_type.clone()))
            .bind(("description", e.description.clone()))
            .bind(("store_id", e.store_id.clone()))
            .bind(("mention_count", e.mention_count))
            .bind(("created_at", e.created_at.clone()))
            .bind(("updated_at", e.updated_at.clone()))
            .await?
            .check()?;
        Ok(())
    }

    async fn get_entity(&self, id: &str) -> Result<Option<Entity>> {
        let mut resp = self
            .db()
            .query("SELECT *, meta::id(id) AS id FROM type::thing('entity', $id)")
            .bind(("id", id.to_string()))
            .await?;
        let rows: Vec<Entity> = resp.take(0)?;
        Ok(rows.into_iter().next())
    }

    async fn list_entities_for_store(&self, store_id: &str) -> Result<Vec<Entity>> {
        let mut resp = self
            .db()
            .query(
                "SELECT *, meta::id(id) AS id FROM entity
                 WHERE store_id = $store_id ORDER BY mention_count DESC",
            )
            .bind(("store_id", store_id.to_string()))
            .await?;
        Ok(resp.take(0)?)
    }

    async fn upsert_entity(&self, e: &Entity) -> Result<()> {
        self.db()
            .query(
                "UPSERT type::thing('entity', $id) CONTENT {
                    name: $name, entity_type: $entity_type,
                    description: $description, store_id: $store_id,
                    mention_count: $mention_count,
                    created_at: $created_at, updated_at: $updated_at
                }",
            )
            .bind(("id", e.id.clone()))
            .bind(("name", e.name.clone()))
            .bind(("entity_type", e.entity_type.clone()))
            .bind(("description", e.description.clone()))
            .bind(("store_id", e.store_id.clone()))
            .bind(("mention_count", e.mention_count))
            .bind(("created_at", e.created_at.clone()))
            .bind(("updated_at", e.updated_at.clone()))
            .await?
            .check()?;
        Ok(())
    }

    async fn upsert_entity_and_increment(&self, e: &Entity) -> Result<()> {
        // Single UPSERT query that atomically creates or updates the entity.
        // SurrealDB does NOT apply schema defaults before SET in UPSERT, so
        // fields are NONE on first creation — the IS NONE guards are required.
        // For new entities: mention_count = $mention_count (caller passes 1).
        // For existing: mention_count increments by 1.
        // created_at is preserved on update (only set on creation).
        self.db()
            .query(
                "UPSERT type::thing('entity', $id) SET
                    name = $name,
                    entity_type = $entity_type,
                    description = $description,
                    store_id = $store_id,
                    mention_count = IF mention_count IS NONE THEN $mention_count ELSE mention_count + 1 END,
                    created_at = IF created_at IS NONE THEN $created_at ELSE created_at END,
                    updated_at = $updated_at",
            )
            .bind(("id", e.id.clone()))
            .bind(("name", e.name.clone()))
            .bind(("entity_type", e.entity_type.clone()))
            .bind(("description", e.description.clone()))
            .bind(("store_id", e.store_id.clone()))
            .bind(("mention_count", e.mention_count))
            .bind(("created_at", e.created_at.clone()))
            .bind(("updated_at", e.updated_at.clone()))
            .await?
            .check()?;
        Ok(())
    }

    // Tag CRUD (P3)
    async fn create_tag(&self, t: &Tag) -> Result<()> {
        self.db()
            .query(
                "CREATE type::thing('tag', $id) CONTENT {
                    name: $name, store_id: $store_id,
                    created_at: $created_at
                }",
            )
            .bind(("id", t.id.clone()))
            .bind(("name", t.name.clone()))
            .bind(("store_id", t.store_id.clone()))
            .bind(("created_at", t.created_at.clone()))
            .await?
            .check()?;
        Ok(())
    }

    async fn list_tags_for_store(&self, store_id: &str) -> Result<Vec<Tag>> {
        let mut resp = self
            .db()
            .query(
                "SELECT *, meta::id(id) AS id FROM tag
                 WHERE store_id = $store_id ORDER BY name",
            )
            .bind(("store_id", store_id.to_string()))
            .await?;
        Ok(resp.take(0)?)
    }

    async fn upsert_tag(&self, t: &Tag) -> Result<()> {
        self.db()
            .query(
                "UPSERT type::thing('tag', $id) CONTENT {
                    name: $name, store_id: $store_id,
                    created_at: $created_at
                }",
            )
            .bind(("id", t.id.clone()))
            .bind(("name", t.name.clone()))
            .bind(("store_id", t.store_id.clone()))
            .bind(("created_at", t.created_at.clone()))
            .await?
            .check()?;
        Ok(())
    }

    // Dedup queue CRUD (P3)
    async fn create_dedup_entry(&self, e: &DedupQueueEntry) -> Result<()> {
        self.db()
            .query(
                "CREATE type::thing('dedup_queue', $id) CONTENT {
                    store_id: $store_id,
                    incoming_title: $incoming_title,
                    incoming_content: $incoming_content,
                    incoming_source_type: $incoming_source_type,
                    incoming_source_id: $incoming_source_id,
                    matched_article_id: $matched_article_id,
                    content_hash: $content_hash,
                    status: $status,
                    created_at: $created_at,
                    resolved_at: $resolved_at
                }",
            )
            .bind(("id", e.id.clone()))
            .bind(("store_id", e.store_id.clone()))
            .bind(("incoming_title", e.incoming_title.clone()))
            .bind(("incoming_content", e.incoming_content.clone()))
            .bind(("incoming_source_type", e.incoming_source_type.clone()))
            .bind(("incoming_source_id", e.incoming_source_id.clone()))
            .bind(("matched_article_id", e.matched_article_id.clone()))
            .bind(("content_hash", e.content_hash.clone()))
            .bind(("status", e.status.clone()))
            .bind(("created_at", e.created_at.clone()))
            .bind(("resolved_at", e.resolved_at.clone()))
            .await?
            .check()?;
        Ok(())
    }

    async fn list_pending_dedup(&self, store_id: &str) -> Result<Vec<DedupQueueEntry>> {
        let mut resp = self
            .db()
            .query(
                "SELECT *, meta::id(id) AS id FROM dedup_queue
                 WHERE store_id = $store_id AND status = 'pending'
                 ORDER BY created_at DESC",
            )
            .bind(("store_id", store_id.to_string()))
            .await?;
        Ok(resp.take(0)?)
    }

    async fn get_dedup_entry(&self, id: &str) -> Result<Option<DedupQueueEntry>> {
        let mut resp = self
            .db()
            .query("SELECT *, meta::id(id) AS id FROM type::thing('dedup_queue', $id)")
            .bind(("id", id.to_string()))
            .await?;
        let rows: Vec<DedupQueueEntry> = resp.take(0)?;
        Ok(rows.into_iter().next())
    }

    async fn resolve_dedup_entry(&self, id: &str, status: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.db()
            .query(
                "UPDATE type::thing('dedup_queue', $id) MERGE {
                    status: $status, resolved_at: $resolved_at
                }",
            )
            .bind(("id", id.to_string()))
            .bind(("status", status.to_string()))
            .bind(("resolved_at", now))
            .await?
            .check()?;
        Ok(())
    }

    // Graph edge methods (P3)
    async fn create_mentions_edge(
        &self, article_id: &str, entity_id: &str, excerpt: &str, confidence: f64,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.db()
            .query(
                "LET $from = type::thing('article', $article_id);
                 LET $to   = type::thing('entity',  $entity_id);
                 DELETE FROM mentions WHERE in = $from AND out = $to;
                 RELATE $from->mentions->$to CONTENT {
                    excerpt: $excerpt,
                    confidence: $confidence,
                    created_at: $now
                }",
            )
            .bind(("article_id", article_id.to_string()))
            .bind(("entity_id", entity_id.to_string()))
            .bind(("excerpt", excerpt.to_string()))
            .bind(("confidence", confidence))
            .bind(("now", now))
            .await?
            .check()?;
        Ok(())
    }

    async fn create_tagged_edge(&self, article_id: &str, tag_id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.db()
            .query(
                "LET $from = type::thing('article', $article_id);
                 LET $to   = type::thing('tag',     $tag_id);
                 DELETE FROM tagged WHERE in = $from AND out = $to;
                 RELATE $from->tagged->$to CONTENT {
                    created_at: $now
                }",
            )
            .bind(("article_id", article_id.to_string()))
            .bind(("tag_id", tag_id.to_string()))
            .bind(("now", now))
            .await?
            .check()?;
        Ok(())
    }

    async fn create_or_update_related_to_edge(
        &self, from_article_id: &str, to_article_id: &str,
        shared_entity_count: i64, strength: f64,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        // Delete existing edge if any, then recreate. This avoids duplicate edge errors.
        self.db()
            .query(
                "LET $from = type::thing('article', $from_id);
                 LET $to   = type::thing('article', $to_id);
                 DELETE FROM related_to WHERE in = $from AND out = $to;
                 RELATE $from->related_to->$to CONTENT {
                    shared_entity_count: $count,
                    strength: $strength,
                    created_at: $now,
                    updated_at: $now
                }",
            )
            .bind(("from_id", from_article_id.to_string()))
            .bind(("to_id", to_article_id.to_string()))
            .bind(("count", shared_entity_count))
            .bind(("strength", strength))
            .bind(("now", now))
            .await?
            .check()?;
        Ok(())
    }

    async fn list_entities_for_article(&self, article_id: &str) -> Result<Vec<Entity>> {
        let mut resp = self
            .db()
            .query(
                "SELECT *, meta::id(id) AS id FROM entity
                 WHERE id IN (
                    SELECT VALUE out FROM mentions
                    WHERE in = type::thing('article', $article_id)
                 )",
            )
            .bind(("article_id", article_id.to_string()))
            .await?;
        Ok(resp.take(0)?)
    }

    async fn list_articles_for_entity(&self, entity_id: &str) -> Result<Vec<Article>> {
        let mut resp = self
            .db()
            .query(
                "SELECT *, meta::id(id) AS id FROM article
                 WHERE id IN (
                    SELECT VALUE in FROM mentions
                    WHERE out = type::thing('entity', $entity_id)
                 )",
            )
            .bind(("entity_id", entity_id.to_string()))
            .await?;
        Ok(resp.take(0)?)
    }

    async fn list_tags_for_article(&self, article_id: &str) -> Result<Vec<Tag>> {
        let mut resp = self
            .db()
            .query(
                "SELECT *, meta::id(id) AS id FROM tag
                 WHERE id IN (
                    SELECT VALUE out FROM tagged
                    WHERE in = type::thing('article', $article_id)
                 )",
            )
            .bind(("article_id", article_id.to_string()))
            .await?;
        Ok(resp.take(0)?)
    }

    async fn list_related_articles(&self, article_id: &str) -> Result<Vec<Article>> {
        // Query both directions since ENTITY_OVERLAP edges are stored unidirectionally
        // (P5 renamed RELATED_TO -> ENTITY_OVERLAP; same Jaccard-on-shared-entities semantics).
        let mut resp = self
            .db()
            .query(
                "SELECT *, meta::id(id) AS id FROM article
                 WHERE id IN (
                    SELECT VALUE out FROM entity_overlap
                    WHERE in = type::thing('article', $article_id)
                 )
                 OR id IN (
                    SELECT VALUE in FROM entity_overlap
                    WHERE out = type::thing('article', $article_id)
                 )",
            )
            .bind(("article_id", article_id.to_string()))
            .await?;
        Ok(resp.take(0)?)
    }

    async fn list_articles_without_mentions(&self, store_id: &str) -> Result<Vec<Article>> {
        let mut resp = self
            .db()
            .query(
                "SELECT *, meta::id(id) AS id FROM article
                 WHERE store_id = $store_id
                 AND id NOT IN (SELECT VALUE in FROM mentions)
                 ORDER BY created_at DESC",
            )
            .bind(("store_id", store_id.to_string()))
            .await?;
        Ok(resp.take(0)?)
    }

    // Graph queries (P4)

    async fn search_entities_by_name(&self, store_id: &str, terms: &[&str]) -> Result<Vec<Entity>> {
        if terms.is_empty() {
            return Ok(vec![]);
        }
        let mut conditions = Vec::new();
        let mut binds: Vec<(String, String)> = Vec::new();
        for (i, term) in terms.iter().enumerate() {
            let lower = term.to_lowercase();
            let param = format!("term_{}", i);
            conditions.push(format!(
                "(string::lowercase(name) = ${p} OR string::lowercase(name) CONTAINS ${p})",
                p = param
            ));
            binds.push((param, lower));
        }
        let where_clause = conditions.join(" OR ");
        let query = format!(
            "SELECT *, meta::id(id) AS id FROM entity WHERE store_id = $store_id AND ({}) ORDER BY mention_count DESC",
            where_clause
        );
        let mut q = self.db().query(&query).bind(("store_id", store_id.to_string()));
        for (param, value) in binds {
            q = q.bind((param, value));
        }
        let mut resp = q.await.context("search_entities_by_name query failed")?;
        let rows: Vec<Entity> = resp.take(0).unwrap_or_default();
        Ok(rows)
    }

    async fn list_articles_for_entities(&self, entity_ids: &[&str]) -> Result<Vec<(Article, f64)>> {
        if entity_ids.is_empty() {
            return Ok(vec![]);
        }

        // Fetch edges for each entity and accumulate max confidence per article.
        // We iterate per entity (small N in practice) to stay within SurrealQL
        // type-safe Thing lookups — passing raw strings in an IN clause does not
        // match SurrealDB Thing values.
        let mut article_confidences: std::collections::HashMap<String, f64> =
            std::collections::HashMap::new();

        for entity_id in entity_ids {
            let mut resp = self
                .db()
                .query(
                    "SELECT
                        meta::id(in) AS article_id,
                        confidence
                     FROM mentions
                     WHERE out = type::thing('entity', $entity_id)",
                )
                .bind(("entity_id", entity_id.to_string()))
                .await
                .context("list_articles_for_entities per-entity query failed")?;
            let edges: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();

            for edge in &edges {
                let aid = edge
                    .get("article_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                let conf = edge
                    .get("confidence")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let entry = article_confidences.entry(aid.to_string()).or_insert(0.0);
                if conf > *entry {
                    *entry = conf;
                }
            }
        }

        let mut results = Vec::new();
        for (aid, confidence) in &article_confidences {
            if let Some(article) = self.get_article(aid).await? {
                results.push((article, *confidence));
            }
        }
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(results)
    }

    async fn count_entities_by_type(
        &self,
        store_id: &str,
    ) -> Result<std::collections::HashMap<String, usize>> {
        let mut resp = self
            .db()
            .query(
                "SELECT entity_type, count() AS count FROM entity
                 WHERE store_id = $store_id
                 GROUP BY entity_type",
            )
            .bind(("store_id", store_id.to_string()))
            .await
            .context("count_entities_by_type query failed")?;
        let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();

        let mut counts = std::collections::HashMap::new();
        for row in rows {
            let etype = row
                .get("entity_type")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let count = row
                .get("count")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            counts.insert(etype, count);
        }
        Ok(counts)
    }

    /// Returns entities that co-occur with the given entity in shared articles.
    ///
    /// The `usize` in each returned tuple is the number of distinct articles
    /// where both the given entity and the co-entity are mentioned. This is
    /// the co-occurrence article count — NOT the co-entity's global mention_count.
    /// Sorted by shared-article count descending.
    async fn list_co_mentioned_entities(
        &self,
        entity_id: &str,
    ) -> Result<Vec<(Entity, usize)>> {
        // Step 1: find all articles that mention the target entity.
        let mut resp = self
            .db()
            .query(
                "SELECT VALUE meta::id(in) AS article_id
                 FROM mentions
                 WHERE out = type::thing('entity', $entity_id)",
            )
            .bind(("entity_id", entity_id.to_string()))
            .await
            .context("list_co_mentioned_entities: step1 failed")?;
        let article_ids: Vec<String> = resp.take(0).unwrap_or_default();

        if article_ids.is_empty() {
            return Ok(vec![]);
        }

        // Step 2: for each co-article, find which OTHER entities it mentions.
        // We accumulate shared-article counts per co-entity in Rust.
        let mut co_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();

        for article_id in &article_ids {
            let mut resp2 = self
                .db()
                .query(
                    "SELECT VALUE meta::id(out) AS entity_id
                     FROM mentions
                     WHERE in = type::thing('article', $article_id)
                       AND out != type::thing('entity', $entity_id)",
                )
                .bind(("article_id", article_id.clone()))
                .bind(("entity_id", entity_id.to_string()))
                .await
                .context("list_co_mentioned_entities: step2 failed")?;
            let co_ids: Vec<String> = resp2.take(0).unwrap_or_default();
            for co_id in co_ids {
                *co_counts.entry(co_id).or_insert(0) += 1;
            }
        }

        // Step 3: fetch entity records and build result.
        let mut results: Vec<(Entity, usize)> = Vec::new();
        for (co_id, count) in &co_counts {
            if let Some(entity) = self.get_entity(co_id).await? {
                results.push((entity, *count));
            }
        }
        results.sort_by(|a, b| b.1.cmp(&a.1));
        Ok(results)
    }

    // P5 typed edge implementations (Task 3)

    async fn create_precedes_edge(
        &self,
        store_id: &str,
        from_article_id: &str,
        to_article_id: &str,
        confidence: f64,
        method: ExtractionMethod,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let method_str = serde_json::to_value(method)
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "heuristic".into());

        let res = self.db()
            .query(
                "LET $from = type::thing('article', $from_id);
                 LET $to = type::thing('article', $to_id);
                 RELATE $from->precedes->$to CONTENT {
                    confidence: $conf,
                    extraction_method: $method,
                    store_id: $sid,
                    created_at: $now
                 }"
            )
            .bind(("from_id", from_article_id.to_string()))
            .bind(("to_id", to_article_id.to_string()))
            .bind(("conf", confidence))
            .bind(("method", method_str))
            .bind(("sid", store_id.to_string()))
            .bind(("now", now))
            .await;
        match res { Ok(r) => { let _ = r.check(); Ok(()) } Err(_) => Ok(()) }
    }

    async fn create_semantically_related_edge(
        &self,
        store_id: &str,
        from_article_id: &str,
        to_article_id: &str,
        similarity: f64,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let res = self.db()
            .query(
                "LET $from = type::thing('article', $from_id);
                 LET $to = type::thing('article', $to_id);
                 RELATE $from->semantically_related->$to CONTENT {
                    similarity: $sim,
                    confidence: $sim,
                    extraction_method: 'derived',
                    store_id: $sid,
                    created_at: $now
                 }"
            )
            .bind(("from_id", from_article_id.to_string()))
            .bind(("to_id", to_article_id.to_string()))
            .bind(("sim", similarity))
            .bind(("sid", store_id.to_string()))
            .bind(("now", now))
            .await;
        match res { Ok(r) => { let _ = r.check(); Ok(()) } Err(_) => Ok(()) }
    }

    async fn create_caused_by_edge(
        &self,
        store_id: &str,
        from_article_id: &str,
        to_article_id: &str,
        confidence: f64,
        rationale: Option<String>,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let res = self.db()
            .query(
                "LET $from = type::thing('article', $from_id);
                 LET $to = type::thing('article', $to_id);
                 RELATE $from->caused_by->$to CONTENT {
                    confidence: $conf,
                    rationale: $rationale,
                    extraction_method: 'llm',
                    store_id: $sid,
                    created_at: $now
                 }"
            )
            .bind(("from_id", from_article_id.to_string()))
            .bind(("to_id", to_article_id.to_string()))
            .bind(("conf", confidence))
            .bind(("rationale", rationale))
            .bind(("sid", store_id.to_string()))
            .bind(("now", now))
            .await;
        match res { Ok(r) => { let _ = r.check(); Ok(()) } Err(_) => Ok(()) }
    }

    async fn create_references_edge(
        &self,
        store_id: &str,
        from_article_id: &str,
        to_article_id: &str,
        anchor_text: Option<String>,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let res = self.db()
            .query(
                "LET $from = type::thing('article', $from_id);
                 LET $to = type::thing('article', $to_id);
                 RELATE $from->references_edge->$to CONTENT {
                    confidence: 1.0,
                    anchor_text: $anchor,
                    extraction_method: 'user_asserted',
                    store_id: $sid,
                    created_at: $now
                 }"
            )
            .bind(("from_id", from_article_id.to_string()))
            .bind(("to_id", to_article_id.to_string()))
            .bind(("anchor", anchor_text))
            .bind(("sid", store_id.to_string()))
            .bind(("now", now))
            .await;
        match res { Ok(r) => { let _ = r.check(); Ok(()) } Err(_) => Ok(()) }
    }

    async fn list_precedes_for(&self, store_id: &str, article_id: &str) -> Result<Vec<PrecedesEdge>> {
        let mut resp = self.db()
            .query(
                "SELECT meta::id(in) AS from_article_id, meta::id(out) AS to_article_id,
                        confidence, extraction_method, created_at
                 FROM precedes
                 WHERE store_id = $sid
                   AND in = type::thing('article', $aid)"
            )
            .bind(("sid", store_id.to_string()))
            .bind(("aid", article_id.to_string()))
            .await?;
        let edges: Vec<PrecedesEdge> = resp.take(0).unwrap_or_default();
        Ok(edges)
    }

    async fn list_semantically_related_for(&self, store_id: &str, article_id: &str) -> Result<Vec<SemanticallyRelatedEdge>> {
        let mut resp = self.db()
            .query(
                "SELECT meta::id(in) AS from_article_id, meta::id(out) AS to_article_id,
                        similarity, confidence, extraction_method, created_at
                 FROM semantically_related
                 WHERE store_id = $sid
                   AND in = type::thing('article', $aid)"
            )
            .bind(("sid", store_id.to_string()))
            .bind(("aid", article_id.to_string()))
            .await?;
        let edges: Vec<SemanticallyRelatedEdge> = resp.take(0).unwrap_or_default();
        Ok(edges)
    }

    async fn list_caused_by_for(&self, store_id: &str, article_id: &str) -> Result<Vec<CausedByEdge>> {
        let mut resp = self.db()
            .query(
                "SELECT meta::id(in) AS from_article_id, meta::id(out) AS to_article_id,
                        confidence, rationale, extraction_method, created_at
                 FROM caused_by
                 WHERE store_id = $sid
                   AND in = type::thing('article', $aid)"
            )
            .bind(("sid", store_id.to_string()))
            .bind(("aid", article_id.to_string()))
            .await?;
        let edges: Vec<CausedByEdge> = resp.take(0).unwrap_or_default();
        Ok(edges)
    }

    async fn list_references_for(&self, store_id: &str, article_id: &str) -> Result<Vec<ReferencesEdgeRow>> {
        let mut resp = self.db()
            .query(
                "SELECT meta::id(in) AS from_article_id, meta::id(out) AS to_article_id,
                        confidence, anchor_text, extraction_method, created_at
                 FROM references_edge
                 WHERE store_id = $sid
                   AND in = type::thing('article', $aid)"
            )
            .bind(("sid", store_id.to_string()))
            .bind(("aid", article_id.to_string()))
            .await?;
        let edges: Vec<ReferencesEdgeRow> = resp.take(0).unwrap_or_default();
        Ok(edges)
    }

    async fn list_entity_overlap_pairs(&self, store_id: &str) -> Result<Vec<(String, String)>> {
        let mut resp = self.db()
            .query(
                "SELECT meta::id(in) AS from_id, meta::id(out) AS to_id
                 FROM entity_overlap
                 WHERE store_id = $sid"
            )
            .bind(("sid", store_id.to_string()))
            .await?;
        #[derive(serde::Deserialize)]
        struct Row { from_id: String, to_id: String }
        let rows: Vec<Row> = resp.take(0).unwrap_or_default();
        Ok(rows.into_iter().map(|r| (r.from_id, r.to_id)).collect())
    }

    async fn list_article_ids(&self, store_id: &str) -> Result<Vec<String>> {
        let mut resp = self.db()
            .query("SELECT meta::id(id) AS id FROM article WHERE store_id = $sid")
            .bind(("sid", store_id.to_string()))
            .await?;
        #[derive(serde::Deserialize)]
        struct Row { id: String }
        let rows: Vec<Row> = resp.take(0).unwrap_or_default();
        Ok(rows.into_iter().map(|r| r.id).collect())
    }

    async fn list_graph_neighbors(
        &self,
        store_id: &str,
        article_id: &str,
        filter: &crate::config::EdgeTypeFilter,
    ) -> Result<Vec<(String, String, f64)>> {
        #[derive(serde::Deserialize)]
        struct Row { neighbor: String, score: f64 }

        let mut results: Vec<(String, String, f64)> = Vec::new();

        // Run one query per enabled edge type and merge results in Rust.
        // SurrealDB 2 does not support UNION across tables in a single statement.
        if filter.entity_overlap {
            let mut resp = self.db()
                .query(
                    "SELECT meta::id(out) AS neighbor, confidence AS score
                     FROM entity_overlap
                     WHERE store_id = $sid AND in = type::thing('article', $aid)",
                )
                .bind(("sid", store_id.to_string()))
                .bind(("aid", article_id.to_string()))
                .await?;
            let rows: Vec<Row> = resp.take(0).unwrap_or_default();
            results.extend(rows.into_iter().map(|r| (r.neighbor, "entity_overlap".into(), r.score)));
        }
        if filter.semantically_related {
            let mut resp = self.db()
                .query(
                    "SELECT meta::id(out) AS neighbor, similarity AS score
                     FROM semantically_related
                     WHERE store_id = $sid AND in = type::thing('article', $aid)",
                )
                .bind(("sid", store_id.to_string()))
                .bind(("aid", article_id.to_string()))
                .await?;
            let rows: Vec<Row> = resp.take(0).unwrap_or_default();
            results.extend(rows.into_iter().map(|r| (r.neighbor, "semantically_related".into(), r.score)));
        }
        if filter.precedes {
            let mut resp = self.db()
                .query(
                    "SELECT meta::id(out) AS neighbor, confidence AS score
                     FROM precedes
                     WHERE store_id = $sid AND in = type::thing('article', $aid)",
                )
                .bind(("sid", store_id.to_string()))
                .bind(("aid", article_id.to_string()))
                .await?;
            let rows: Vec<Row> = resp.take(0).unwrap_or_default();
            results.extend(rows.into_iter().map(|r| (r.neighbor, "precedes".into(), r.score)));
        }
        if filter.caused_by {
            let mut resp = self.db()
                .query(
                    "SELECT meta::id(out) AS neighbor, confidence AS score
                     FROM caused_by
                     WHERE store_id = $sid AND in = type::thing('article', $aid)",
                )
                .bind(("sid", store_id.to_string()))
                .bind(("aid", article_id.to_string()))
                .await?;
            let rows: Vec<Row> = resp.take(0).unwrap_or_default();
            results.extend(rows.into_iter().map(|r| (r.neighbor, "caused_by".into(), r.score)));
        }
        if filter.references {
            let mut resp = self.db()
                .query(
                    "SELECT meta::id(out) AS neighbor, confidence AS score
                     FROM references_edge
                     WHERE store_id = $sid AND in = type::thing('article', $aid)",
                )
                .bind(("sid", store_id.to_string()))
                .bind(("aid", article_id.to_string()))
                .await?;
            let rows: Vec<Row> = resp.take(0).unwrap_or_default();
            results.extend(rows.into_iter().map(|r| (r.neighbor, "references_edge".into(), r.score)));
        }

        Ok(results)
    }

    async fn count_edges_by_type(&self, store_id: &str) -> Result<EdgeCounts> {
        async fn count_one(db: &surrealdb::Surreal<surrealdb::engine::any::Any>, table: &str, sid: &str) -> Result<i64> {
            let q = format!("SELECT count() AS n FROM {} WHERE store_id = $sid GROUP ALL", table);
            let mut resp = db.query(&q).bind(("sid", sid.to_string())).await?;
            #[derive(serde::Deserialize)] struct C { n: i64 }
            let rows: Vec<C> = resp.take(0).unwrap_or_default();
            Ok(rows.first().map(|c| c.n).unwrap_or(0))
        }

        Ok(EdgeCounts {
            entity_overlap:       count_one(self.db(), "entity_overlap", store_id).await?,
            semantically_related: count_one(self.db(), "semantically_related", store_id).await?,
            precedes:             count_one(self.db(), "precedes", store_id).await?,
            caused_by:            count_one(self.db(), "caused_by", store_id).await?,
            references_edge:      count_one(self.db(), "references_edge", store_id).await?,
        })
    }
    async fn count_mentions_per_entity(&self, store_id: &str) -> Result<std::collections::HashMap<String, usize>> {
        // Get all entities in this store; for each, count incoming MENTIONS edges.
        // Two-step approach (SurrealDB 2 doesn't reliably do GROUP BY across relations).
        let mut resp = self.db()
            .query("SELECT meta::id(id) AS id FROM entity WHERE store_id = $sid")
            .bind(("sid", store_id.to_string()))
            .await
            .context("count_mentions_per_entity: list entities")?;
        #[derive(serde::Deserialize)] struct EntId { id: String }
        let entity_ids: Vec<EntId> = resp.take(0).unwrap_or_default();

        let mut out = std::collections::HashMap::new();
        for ent in entity_ids {
            let ent_id = ent.id.clone();
            let mut cresp = self.db()
                .query("SELECT count() AS cnt FROM mentions WHERE out = type::thing('entity', $eid) GROUP ALL")
                .bind(("eid", ent.id.clone()))
                .await
                .context("count_mentions_per_entity: count per entity")?;
            #[derive(serde::Deserialize)] struct Cnt { cnt: i64 }
            let rows: Vec<Cnt> = cresp.take(0).unwrap_or_default();
            let n = rows.first().map(|r| r.cnt as usize).unwrap_or(0);
            out.insert(ent_id, n);
        }
        Ok(out)
    }
}

/// Convenience alias used across the codebase.
pub type DynStore = dyn Store;

/// Boxed, arc'd instance. Handed into every service that used to hold
/// `Arc<Database>`.
pub type SharedStore = Arc<DynStore>;

/// Wrap a concrete `SurrealStore` into the shared trait object.
pub fn shared(store: SurrealStore) -> SharedStore {
    Arc::new(store)
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
mod fts_tests {
    use super::*;

    fn now() -> String { chrono::Utc::now().to_rfc3339() }

    #[tokio::test]
    async fn test_fts_matches_title_and_content() {
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
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        for (id, title, content) in [
            ("a1", "Rust ownership", "The borrow checker enforces rules."),
            ("a2", "Vector databases", "LanceDB provides columnar storage."),
            ("a3", "Async Rust", "Tokio is a popular runtime for async."),
        ] {
            s.create_article(&Article {
                id: id.into(), store_id: "s1".into(),
                title: title.into(), content: content.into(),
                source_type: "user".into(), source_id: "".into(),
                content_hash: format!("hash-{id}"),
                tags: serde_json::json!([]),
                embedded_at: None,
                created_at: ts.clone(), updated_at: ts.clone(),
            }).await.unwrap();
        }

        let hits = s.fts_search_articles("rust", 10).await.unwrap();
        let ids: Vec<String> = hits.iter().map(|a| a.id.clone()).collect();
        assert!(ids.contains(&"a1".to_string()));
        assert!(ids.contains(&"a3".to_string()));
        assert!(!ids.contains(&"a2".to_string()));

        let hits = s.fts_search_articles("LanceDB", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "a2");
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

#[cfg(test)]
mod entity_tests {
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
    async fn test_entity_crud() {
        let s = fixture().await;
        let ts = now();
        let entity = Entity {
            id: "tool:rust".into(),
            name: "Rust".into(),
            entity_type: "tool".into(),
            description: Some("Systems programming language".into()),
            store_id: "s1".into(),
            mention_count: 0,
            created_at: ts.clone(),
            updated_at: ts.clone(),
        };
        s.create_entity(&entity).await.unwrap();

        let got = s.get_entity("tool:rust").await.unwrap().expect("exists");
        assert_eq!(got.name, "Rust");
        assert_eq!(got.entity_type, "tool");

        let list = s.list_entities_for_store("s1").await.unwrap();
        assert_eq!(list.len(), 1);

        // Upsert: increment mention_count
        let mut updated = entity.clone();
        updated.mention_count = 1;
        updated.updated_at = now();
        s.upsert_entity(&updated).await.unwrap();
        let got = s.get_entity("tool:rust").await.unwrap().unwrap();
        assert_eq!(got.mention_count, 1);
    }

    #[tokio::test]
    async fn test_upsert_entity_and_increment() {
        let s = fixture().await;
        let ts = now();
        let entity = Entity {
            id: "tool:rust".into(),
            name: "Rust".into(),
            entity_type: "tool".into(),
            description: Some("Systems language".into()),
            store_id: "s1".into(),
            mention_count: 1,
            created_at: ts.clone(),
            updated_at: ts.clone(),
        };

        // First call: creates entity with mention_count = 1 (0 default + 1)
        s.upsert_entity_and_increment(&entity).await.unwrap();
        let got = s.get_entity("tool:rust").await.unwrap().expect("exists");
        assert_eq!(got.mention_count, 1);
        let original_created = got.created_at.clone();

        // Second call: increments to 2, preserves created_at
        s.upsert_entity_and_increment(&entity).await.unwrap();
        let got = s.get_entity("tool:rust").await.unwrap().unwrap();
        assert_eq!(got.mention_count, 2);
        assert_eq!(got.created_at, original_created);

        // Third call: increments to 3
        s.upsert_entity_and_increment(&entity).await.unwrap();
        let got = s.get_entity("tool:rust").await.unwrap().unwrap();
        assert_eq!(got.mention_count, 3);
    }

    #[tokio::test]
    async fn test_search_entities_by_name() {
        let s = fixture().await;
        let ts = now();

        // Use unique IDs with "srch-" prefix to avoid collision with parallel tests
        // that may also create tool:rust / tool:tokio in the shared in-memory store.
        s.create_entity(&Entity {
            id: "srch:rust".into(), name: "srch-Rust".into(), entity_type: "tool".into(),
            description: Some("Systems language".into()), store_id: "s1".into(),
            mention_count: 5, created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_entity(&Entity {
            id: "srch:tokio".into(), name: "srch-Tokio".into(), entity_type: "tool".into(),
            description: Some("Async runtime".into()), store_id: "s1".into(),
            mention_count: 3, created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_entity(&Entity {
            id: "srch:async-rt".into(), name: "srch-async runtime".into(),
            entity_type: "concept".into(), description: None, store_id: "s1".into(),
            mention_count: 2, created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        let results = s.search_entities_by_name("s1", &["srch-Rust"]).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "srch:rust");

        let results = s.search_entities_by_name("s1", &["srch-Tok"]).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "srch:tokio");

        let results = s.search_entities_by_name("s1", &["srch-async"]).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "srch:async-rt");

        let results = s.search_entities_by_name("s1", &["srch-Python"]).await.unwrap();
        assert!(results.is_empty());

        let results = s.search_entities_by_name("s1", &["srch-Rust", "srch-Tokio"]).await.unwrap();
        assert_eq!(results.len(), 2);

        let results = s.search_entities_by_name("s2", &["srch-Rust"]).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_list_articles_for_entities() {
        let s = fixture().await;
        let ts = now();

        // Use unique IDs with "lafe-" prefix to avoid collision with parallel tests.
        s.create_article(&Article {
            id: "lafe-a1".into(), store_id: "s1".into(), title: "Rust Guide".into(),
            content: "About Rust".into(), source_type: "user".into(),
            source_id: String::new(), content_hash: "lafe-h1".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_article(&Article {
            id: "lafe-a2".into(), store_id: "s1".into(), title: "Tokio Deep Dive".into(),
            content: "About Tokio".into(), source_type: "user".into(),
            source_id: String::new(), content_hash: "lafe-h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        s.create_entity(&Entity {
            id: "lafe:rust".into(), name: "lafe-Rust".into(), entity_type: "tool".into(),
            description: None, store_id: "s1".into(), mention_count: 1,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        s.create_mentions_edge("lafe-a1", "lafe:rust", "written in Rust", 0.95).await.unwrap();
        s.create_mentions_edge("lafe-a2", "lafe:rust", "uses Rust", 0.80).await.unwrap();

        let results = s.list_articles_for_entities(&["lafe:rust"]).await.unwrap();
        assert_eq!(results.len(), 2);
        // Sorted by confidence desc: a1 (0.95) before a2 (0.80)
        assert_eq!(results[0].0.id, "lafe-a1");
        assert!((results[0].1 - 0.95).abs() < 0.01);
        assert_eq!(results[1].0.id, "lafe-a2");
        assert!((results[1].1 - 0.80).abs() < 0.01);

        let results = s.list_articles_for_entities(&[]).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_count_entities_by_type() {
        // Use a unique store id to avoid data from parallel tests affecting counts.
        let s = SurrealStore::open_in_memory().await.unwrap();
        let ts = now();
        s.create_user(&User {
            id: "cebt-u1".into(), username: "cebt-alice".into(), display_name: "Alice".into(),
            is_owner: true, settings: serde_json::json!({}),
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_store(&KnowledgeStore {
            id: "cebt-s1".into(), owner_id: "cebt-u1".into(), store_type: "personal".into(),
            name: "Notes".into(), lancedb_collection: "store_cebt_s1".into(),
            quantizer_version: "ivf_pq_v1".into(),
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        s.create_entity(&Entity {
            id: "cebt:rust".into(), name: "cebt-Rust".into(), entity_type: "tool".into(),
            description: None, store_id: "cebt-s1".into(), mention_count: 1,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_entity(&Entity {
            id: "cebt:tokio".into(), name: "cebt-Tokio".into(), entity_type: "tool".into(),
            description: None, store_id: "cebt-s1".into(), mention_count: 1,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_entity(&Entity {
            id: "cebt:linus".into(), name: "cebt-Linus".into(), entity_type: "person".into(),
            description: None, store_id: "cebt-s1".into(), mention_count: 1,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        let counts = s.count_entities_by_type("cebt-s1").await.unwrap();
        assert_eq!(counts.get("tool"), Some(&2));
        assert_eq!(counts.get("person"), Some(&1));
        assert_eq!(counts.get("concept"), None);
    }

    #[tokio::test]
    async fn test_list_co_mentioned_entities() {
        // Fresh independent store to avoid any shared-state pollution.
        let s = SurrealStore::open_in_memory().await.unwrap();
        let ts = now();
        s.create_user(&User {
            id: "co-u1".into(), username: "co-alice".into(), display_name: "Alice".into(),
            is_owner: true, settings: serde_json::json!({}),
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_store(&KnowledgeStore {
            id: "co-s1".into(), owner_id: "co-u1".into(), store_type: "personal".into(),
            name: "Notes".into(), lancedb_collection: "store-co-s1".into(),
            quantizer_version: "ivf_pq_v1".into(),
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        s.create_article(&Article {
            id: "co-art1".into(), store_id: "co-s1".into(), title: "Rust Async".into(),
            content: "C".into(), source_type: "user".into(),
            source_id: String::new(), content_hash: "co-hx1".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_article(&Article {
            id: "co-art2".into(), store_id: "co-s1".into(), title: "More Rust".into(),
            content: "C".into(), source_type: "user".into(),
            source_id: String::new(), content_hash: "co-hx2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        // Entity IDs that don't contain colons, to avoid any SurrealDB ID-parsing ambiguity.
        s.create_entity(&Entity {
            id: "co-ent-rust".into(), name: "co-Rust".into(), entity_type: "tool".into(),
            description: None, store_id: "co-s1".into(), mention_count: 1,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_entity(&Entity {
            id: "co-ent-tokio".into(), name: "co-Tokio".into(), entity_type: "tool".into(),
            description: None, store_id: "co-s1".into(), mention_count: 1,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_entity(&Entity {
            id: "co-ent-async".into(), name: "co-async".into(), entity_type: "concept".into(),
            description: None, store_id: "co-s1".into(), mention_count: 1,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        s.create_mentions_edge("co-art1", "co-ent-rust", "e", 0.9).await.unwrap();
        s.create_mentions_edge("co-art1", "co-ent-tokio", "e", 0.9).await.unwrap();
        s.create_mentions_edge("co-art2", "co-ent-rust", "e", 0.9).await.unwrap();
        s.create_mentions_edge("co-art2", "co-ent-async", "e", 0.9).await.unwrap();

        let co = s.list_co_mentioned_entities("co-ent-rust").await.unwrap();
        assert_eq!(co.len(), 2);
        assert!(co.iter().all(|(_, count)| *count == 1));
    }

    #[tokio::test]
    async fn create_precedes_edge_round_trips() {
        let s = fixture().await;
        let ts = now();

        s.create_article(&Article {
            id: "p5pre-a1".into(), store_id: "p5pre-s1".into(), title: "A".into(), content: "x".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "p5pre-h1".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_article(&Article {
            id: "p5pre-a2".into(), store_id: "p5pre-s1".into(), title: "B".into(), content: "y".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "p5pre-h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        s.create_precedes_edge(
            "p5pre-s1", "p5pre-a1", "p5pre-a2",
            1.0, ExtractionMethod::Heuristic,
        ).await.expect("create precedes");

        let edges = s.list_precedes_for("p5pre-s1", "p5pre-a1").await.expect("list");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to_article_id, "p5pre-a2");
        assert_eq!(edges[0].extraction_method, ExtractionMethod::Heuristic);
    }

    #[tokio::test]
    async fn create_semantically_related_edge_dedups_on_unique() {
        let s = fixture().await;
        let ts = now();
        s.create_article(&Article {
            id: "p5sem-a1".into(), store_id: "p5sem-s1".into(), title: "A".into(), content: "".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "p5sem-h1".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_article(&Article {
            id: "p5sem-a2".into(), store_id: "p5sem-s1".into(), title: "B".into(), content: "".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "p5sem-h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        s.create_semantically_related_edge("p5sem-s1", "p5sem-a1", "p5sem-a2", 0.91).await.expect("first");
        // Second insert of same pair should be a no-op (UNIQUE index)
        let res = s.create_semantically_related_edge("p5sem-s1", "p5sem-a1", "p5sem-a2", 0.95).await;
        assert!(res.is_ok(), "duplicate insert should not error; got {:?}", res);

        let edges = s.list_semantically_related_for("p5sem-s1", "p5sem-a1").await.expect("list");
        assert_eq!(edges.len(), 1);
    }

    #[tokio::test]
    async fn create_caused_by_edge_round_trips() {
        let s = fixture().await;
        let ts = now();
        s.create_article(&Article {
            id: "p5cb-a1".into(), store_id: "p5cb-s1".into(), title: "A".into(), content: "".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "p5cb-h1".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_article(&Article {
            id: "p5cb-a2".into(), store_id: "p5cb-s1".into(), title: "B".into(), content: "".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "p5cb-h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        s.create_caused_by_edge(
            "p5cb-s1", "p5cb-a1", "p5cb-a2",
            0.82, Some("explicit 'because' clause".into()),
        ).await.expect("create caused_by");

        let edges = s.list_caused_by_for("p5cb-s1", "p5cb-a1").await.expect("list");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].rationale.as_deref(), Some("explicit 'because' clause"));
    }

    #[tokio::test]
    async fn create_references_edge_round_trips() {
        let s = fixture().await;
        let ts = now();
        s.create_article(&Article {
            id: "p5ref-a1".into(), store_id: "p5ref-s1".into(), title: "A".into(), content: "".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "p5ref-h1".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_article(&Article {
            id: "p5ref-a2".into(), store_id: "p5ref-s1".into(), title: "B".into(), content: "".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "p5ref-h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        s.create_references_edge(
            "p5ref-s1", "p5ref-a1", "p5ref-a2",
            Some("see [related](p5ref-a2)".into()),
        ).await.expect("create references");

        let edges = s.list_references_for("p5ref-s1", "p5ref-a1").await.expect("list");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].anchor_text.as_deref(), Some("see [related](p5ref-a2)"));
    }

    #[tokio::test]
    async fn list_graph_neighbors_default_filter_traverses_entity_overlap_only() {
        let s = SurrealStore::open_in_memory().await.expect("open mem");
        s.db().query(r#"
            CREATE article:lgn1a CONTENT { store_id: "lgn1-s1", title: "A", content: "",
                source_type: "user", source_id: "", content_hash: "lgn1-a", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:lgn1b CONTENT { store_id: "lgn1-s1", title: "B", content: "",
                source_type: "user", source_id: "", content_hash: "lgn1-b", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:lgn1c CONTENT { store_id: "lgn1-s1", title: "C", content: "",
                source_type: "user", source_id: "", content_hash: "lgn1-c", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            RELATE article:lgn1a->entity_overlap->article:lgn1b CONTENT {
                shared_entity_count: 2, strength: 0.5, confidence: 0.5,
                extraction_method: "heuristic", store_id: "lgn1-s1",
                created_at: "2026-01-01T00:00:01Z", updated_at: "2026-01-01T00:00:01Z"
            };
            RELATE article:lgn1a->semantically_related->article:lgn1c CONTENT {
                similarity: 0.9, confidence: 0.9,
                extraction_method: "derived", store_id: "lgn1-s1",
                created_at: "2026-01-01T00:00:02Z"
            };
        "#).await.expect("seed").check().expect("seed check");

        // Default filter: only entity_overlap traversed
        let filter = crate::config::EdgeTypeFilter::default();
        let neighbors = s.list_graph_neighbors("lgn1-s1", "lgn1a", &filter).await.expect("traverse");
        assert_eq!(neighbors.len(), 1, "default filter returns only entity_overlap neighbor");
        assert_eq!(neighbors[0].0, "lgn1b");
        assert_eq!(neighbors[0].1, "entity_overlap");
    }

    #[tokio::test]
    async fn list_graph_neighbors_with_semantic_enabled_returns_both() {
        let s = SurrealStore::open_in_memory().await.expect("open mem");
        s.db().query(r#"
            CREATE article:lgn2a CONTENT { store_id: "lgn2-s1", title: "A", content: "",
                source_type: "user", source_id: "", content_hash: "lgn2-a", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:lgn2b CONTENT { store_id: "lgn2-s1", title: "B", content: "",
                source_type: "user", source_id: "", content_hash: "lgn2-b", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:lgn2c CONTENT { store_id: "lgn2-s1", title: "C", content: "",
                source_type: "user", source_id: "", content_hash: "lgn2-c", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            RELATE article:lgn2a->entity_overlap->article:lgn2b CONTENT {
                shared_entity_count: 2, strength: 0.5, confidence: 0.5,
                extraction_method: "heuristic", store_id: "lgn2-s1",
                created_at: "2026-01-01T00:00:01Z", updated_at: "2026-01-01T00:00:01Z"
            };
            RELATE article:lgn2a->semantically_related->article:lgn2c CONTENT {
                similarity: 0.9, confidence: 0.9,
                extraction_method: "derived", store_id: "lgn2-s1",
                created_at: "2026-01-01T00:00:02Z"
            };
        "#).await.expect("seed").check().expect("seed check");

        let filter = crate::config::EdgeTypeFilter {
            semantically_related: true,
            ..Default::default()
        };
        let neighbors = s.list_graph_neighbors("lgn2-s1", "lgn2a", &filter).await.expect("traverse");
        assert_eq!(neighbors.len(), 2);
        let types: std::collections::HashSet<&str> = neighbors.iter().map(|n| n.1.as_str()).collect();
        assert!(types.contains("entity_overlap"));
        assert!(types.contains("semantically_related"));
    }
}

#[cfg(test)]
mod tag_tests {
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
    async fn test_tag_crud() {
        let s = fixture().await;
        let ts = now();
        let tag = Tag {
            id: "machine-learning".into(),
            name: "Machine Learning".into(),
            store_id: "s1".into(),
            created_at: ts.clone(),
        };
        s.create_tag(&tag).await.unwrap();

        let list = s.list_tags_for_store("s1").await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, "Machine Learning");

        // Upsert same tag should not fail
        s.upsert_tag(&tag).await.unwrap();
        assert_eq!(s.list_tags_for_store("s1").await.unwrap().len(), 1);
    }
}

#[cfg(test)]
mod dedup_tests {
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
    async fn test_dedup_queue_crud() {
        let s = fixture().await;
        let ts = now();
        let entry = DedupQueueEntry {
            id: "dq1".into(),
            store_id: "s1".into(),
            incoming_title: "Dup Article".into(),
            incoming_content: "Some content".into(),
            incoming_source_type: "user".into(),
            incoming_source_id: None,
            matched_article_id: "a1".into(),
            content_hash: "hash123".into(),
            status: "pending".into(),
            created_at: ts.clone(),
            resolved_at: None,
        };
        s.create_dedup_entry(&entry).await.unwrap();

        let pending = s.list_pending_dedup("s1").await.unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].incoming_title, "Dup Article");

        // Resolve
        s.resolve_dedup_entry("dq1", "rejected").await.unwrap();
        let pending = s.list_pending_dedup("s1").await.unwrap();
        assert_eq!(pending.len(), 0);
    }
}

#[cfg(test)]
mod graph_edge_tests {
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
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_article(&Article {
            id: "a1".into(), store_id: "s1".into(), title: "Rust Guide".into(),
            content: "Learn Rust".into(), source_type: "user".into(),
            source_id: "".into(), content_hash: "h1".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_article(&Article {
            id: "a2".into(), store_id: "s1".into(), title: "Async Rust".into(),
            content: "Tokio and async".into(), source_type: "user".into(),
            source_id: "".into(), content_hash: "h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_entity(&Entity {
            id: "tool:rust".into(), name: "Rust".into(), entity_type: "tool".into(),
            description: None, store_id: "s1".into(), mention_count: 0,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_tag(&Tag {
            id: "programming".into(), name: "Programming".into(),
            store_id: "s1".into(), created_at: ts,
        }).await.unwrap();
        s
    }

    #[tokio::test]
    async fn test_mentions_edge() {
        let s = fixture().await;
        s.create_mentions_edge("a1", "tool:rust", "written in Rust", 0.95).await.unwrap();

        let entities = s.list_entities_for_article("a1").await.unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].name, "Rust");

        let articles = s.list_articles_for_entity("tool:rust").await.unwrap();
        assert_eq!(articles.len(), 1);
        assert_eq!(articles[0].id, "a1");
    }

    #[tokio::test]
    async fn test_tagged_edge() {
        let s = fixture().await;
        s.create_tagged_edge("a1", "programming").await.unwrap();

        let tags = s.list_tags_for_article("a1").await.unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "Programming");
    }

    #[tokio::test]
    async fn test_related_to_edge() {
        // Seed entity_overlap directly (P5: related_to was renamed to entity_overlap).
        let s = fixture().await;
        let now = chrono::Utc::now().to_rfc3339();
        s.db().query(
            "RELATE article:a1->entity_overlap->article:a2 CONTENT {
                shared_entity_count: 1, strength: 0.5, confidence: 0.5,
                extraction_method: 'heuristic', store_id: 's1',
                created_at: $now, updated_at: $now
            }",
        )
        .bind(("now", now.clone()))
        .await.unwrap().check().unwrap();

        let related = s.list_related_articles("a1").await.unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].id, "a2");

        // Update edge (delete + recreate to change count) — still 1 edge
        s.db().query(
            "DELETE FROM entity_overlap WHERE in = type::thing('article', 'a1') AND out = type::thing('article', 'a2');
             RELATE article:a1->entity_overlap->article:a2 CONTENT {
                shared_entity_count: 2, strength: 0.8, confidence: 0.8,
                extraction_method: 'heuristic', store_id: 's1',
                created_at: $now, updated_at: $now
            }",
        )
        .bind(("now", now))
        .await.unwrap().check().unwrap();
        let related = s.list_related_articles("a1").await.unwrap();
        assert_eq!(related.len(), 1);
    }
}

#[cfg(test)]
mod p3_integration_tests {
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
    async fn test_full_graph_pipeline() {
        let s = fixture().await;
        let ts = now();

        // Create two articles
        for (id, title, content, hash) in [
            ("a1", "Rust Async", "Tokio and Rust async programming", "h1"),
            ("a2", "Systems Rust", "Rust for systems programming with Tokio", "h2"),
        ] {
            s.create_article(&Article {
                id: id.into(), store_id: "s1".into(), title: title.into(),
                content: content.into(), source_type: "user".into(),
                source_id: "".into(), content_hash: hash.into(),
                tags: serde_json::json!([]), embedded_at: None,
                created_at: ts.clone(), updated_at: ts.clone(),
            }).await.unwrap();
        }

        // Create entities
        let rust_entity = Entity {
            id: "tool:rust".into(), name: "Rust".into(), entity_type: "tool".into(),
            description: Some("Systems language".into()), store_id: "s1".into(),
            mention_count: 2, created_at: ts.clone(), updated_at: ts.clone(),
        };
        s.upsert_entity(&rust_entity).await.unwrap();

        let tokio_entity = Entity {
            id: "tool:tokio".into(), name: "Tokio".into(), entity_type: "tool".into(),
            description: Some("Async runtime".into()), store_id: "s1".into(),
            mention_count: 2, created_at: ts.clone(), updated_at: ts.clone(),
        };
        s.upsert_entity(&tokio_entity).await.unwrap();

        // Create MENTIONS edges
        s.create_mentions_edge("a1", "tool:rust", "Rust async", 0.9).await.unwrap();
        s.create_mentions_edge("a1", "tool:tokio", "Tokio and", 0.9).await.unwrap();
        s.create_mentions_edge("a2", "tool:rust", "Rust for systems", 0.9).await.unwrap();
        s.create_mentions_edge("a2", "tool:tokio", "with Tokio", 0.85).await.unwrap();

        // Verify entity lookup
        let entities_a1 = s.list_entities_for_article("a1").await.unwrap();
        assert_eq!(entities_a1.len(), 2);

        let articles_rust = s.list_articles_for_entity("tool:rust").await.unwrap();
        assert_eq!(articles_rust.len(), 2);

        // Create ENTITY_OVERLAP edge: a1 and a2 share 2 entities out of 2+2-2=2, strength=1.0
        // (P5: related_to was renamed to entity_overlap)
        let now_ts = now();
        s.db().query(
            "RELATE article:a1->entity_overlap->article:a2 CONTENT {
                shared_entity_count: 2, strength: 1.0, confidence: 1.0,
                extraction_method: 'heuristic', store_id: 's1',
                created_at: $now, updated_at: $now
            }",
        )
        .bind(("now", now_ts))
        .await.unwrap().check().unwrap();

        let related = s.list_related_articles("a1").await.unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].id, "a2");
    }

    #[tokio::test]
    async fn test_dedup_and_review_flow() {
        let s = fixture().await;
        let ts = now();

        // Create original article
        s.create_article(&Article {
            id: "a1".into(), store_id: "s1".into(), title: "Original".into(),
            content: "Original content".into(), source_type: "user".into(),
            source_id: "".into(), content_hash: "hash1".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        // Simulate dedup detection
        let dup_entry = DedupQueueEntry {
            id: "dq1".into(), store_id: "s1".into(),
            incoming_title: "Duplicate".into(),
            incoming_content: "Original content".into(),
            incoming_source_type: "user".into(),
            incoming_source_id: None,
            matched_article_id: "a1".into(),
            content_hash: "hash1".into(),
            status: "pending".into(),
            created_at: ts.clone(), resolved_at: None,
        };
        s.create_dedup_entry(&dup_entry).await.unwrap();

        // List pending
        let pending = s.list_pending_dedup("s1").await.unwrap();
        assert_eq!(pending.len(), 1);

        // Reject
        s.resolve_dedup_entry("dq1", "rejected").await.unwrap();
        assert_eq!(s.list_pending_dedup("s1").await.unwrap().len(), 0);

        let resolved = s.get_dedup_entry("dq1").await.unwrap().unwrap();
        assert_eq!(resolved.status, "rejected");
        assert!(resolved.resolved_at.is_some());
    }

    #[tokio::test]
    async fn test_tag_migration_flow() {
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
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        // Create tag and tagged edge manually (simulating post-migration state)
        s.upsert_tag(&Tag {
            id: "rust".into(), name: "rust".into(),
            store_id: "s1".into(), created_at: ts.clone(),
        }).await.unwrap();
        s.create_article(&Article {
            id: "a1".into(), store_id: "s1".into(), title: "Test".into(),
            content: "Content".into(), source_type: "user".into(),
            source_id: "".into(), content_hash: "h1".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_tagged_edge("a1", "rust").await.unwrap();

        let tags = s.list_tags_for_article("a1").await.unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].id, "rust");

        let store_tags = s.list_tags_for_store("s1").await.unwrap();
        assert_eq!(store_tags.len(), 1);
    }

    #[tokio::test]
    async fn test_list_articles_without_mentions() {
        let s = fixture().await;
        let ts = now();

        s.create_article(&Article {
            id: "a1".into(), store_id: "s1".into(), title: "Has mentions".into(),
            content: "C1".into(), source_type: "user".into(),
            source_id: "".into(), content_hash: "h1".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_article(&Article {
            id: "a2".into(), store_id: "s1".into(), title: "No mentions".into(),
            content: "C2".into(), source_type: "user".into(),
            source_id: "".into(), content_hash: "h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        s.upsert_entity(&Entity {
            id: "tool:rust".into(), name: "Rust".into(), entity_type: "tool".into(),
            description: None, store_id: "s1".into(), mention_count: 1,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_mentions_edge("a1", "tool:rust", "in Rust", 0.9).await.unwrap();

        let without = s.list_articles_without_mentions("s1").await.unwrap();
        assert_eq!(without.len(), 1);
        assert_eq!(without[0].id, "a2");
    }

    /// Tests the store-level graph query methods used by GraphSearcher:
    /// search_entities_by_name, list_articles_for_entities, list_co_mentioned_entities.
    #[tokio::test]
    async fn test_graph_store_queries_for_searcher() {
        let s = fixture().await;
        let ts = now();

        // Create three articles
        s.create_article(&Article {
            id: "tgsi-a1".into(), store_id: "s1".into(), title: "Rust Async Programming".into(),
            content: "Rust provides powerful async capabilities using Tokio runtime".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "tgsi-h1".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_article(&Article {
            id: "tgsi-a2".into(), store_id: "s1".into(), title: "Go Concurrency".into(),
            content: "Go uses goroutines for concurrent programming".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "tgsi-h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_article(&Article {
            id: "tgsi-a3".into(), store_id: "s1".into(), title: "Tokio Internals".into(),
            content: "Deep dive into how Tokio scheduler works".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "tgsi-h3".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        // Create entities
        s.create_entity(&Entity {
            id: "tgsi-tool-rust".into(), name: "Rust".into(), entity_type: "tool".into(),
            description: Some("Systems programming language".into()), store_id: "s1".into(),
            mention_count: 2, created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_entity(&Entity {
            id: "tgsi-tool-tokio".into(), name: "Tokio".into(), entity_type: "tool".into(),
            description: Some("Async runtime for Rust".into()), store_id: "s1".into(),
            mention_count: 2, created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        // Create MENTIONS edges
        s.create_mentions_edge("tgsi-a1", "tgsi-tool-rust", "Rust provides", 0.95).await.unwrap();
        s.create_mentions_edge("tgsi-a1", "tgsi-tool-tokio", "using Tokio", 0.90).await.unwrap();
        s.create_mentions_edge("tgsi-a3", "tgsi-tool-tokio", "Tokio scheduler", 0.92).await.unwrap();

        // Create ENTITY_OVERLAP edge (a1 and a3 share tokio)
        // (P5: related_to was renamed to entity_overlap)
        s.db().query(
            "LET $from = type::thing('article', $from_id);
             LET $to   = type::thing('article', $to_id);
             RELATE $from->entity_overlap->$to CONTENT {
                shared_entity_count: 1, strength: 0.5, confidence: 0.5,
                extraction_method: 'heuristic', store_id: 's1',
                created_at: '2026-01-01T00:00:00Z', updated_at: '2026-01-01T00:00:00Z'
             }",
        )
        .bind(("from_id", "tgsi-a1".to_string()))
        .bind(("to_id", "tgsi-a3".to_string()))
        .await.unwrap().check().unwrap();

        // search_entities_by_name: "Rust" should match tgsi-tool-rust
        let rust_matches = s.search_entities_by_name("s1", &["Rust"]).await.unwrap();
        assert!(!rust_matches.is_empty());
        assert!(rust_matches.iter().any(|e| e.id == "tgsi-tool-rust"));

        // search_entities_by_name: "Tokio" should match tgsi-tool-tokio
        let tokio_matches = s.search_entities_by_name("s1", &["Tokio"]).await.unwrap();
        assert!(!tokio_matches.is_empty());
        assert!(tokio_matches.iter().any(|e| e.id == "tgsi-tool-tokio"));

        // search_entities_by_name: "Go" should find nothing (no entity)
        let go_matches = s.search_entities_by_name("s1", &["Go"]).await.unwrap();
        assert!(go_matches.is_empty());

        // list_articles_for_entities: Rust entity should yield a1
        let rust_articles = s.list_articles_for_entities(&["tgsi-tool-rust"]).await.unwrap();
        assert_eq!(rust_articles.len(), 1);
        assert_eq!(rust_articles[0].0.id, "tgsi-a1");

        // list_articles_for_entities: Tokio entity should yield a1 and a3
        let tokio_articles = s.list_articles_for_entities(&["tgsi-tool-tokio"]).await.unwrap();
        assert_eq!(tokio_articles.len(), 2);
        let ids: Vec<&str> = tokio_articles.iter().map(|(a, _)| a.id.as_str()).collect();
        assert!(ids.contains(&"tgsi-a1"));
        assert!(ids.contains(&"tgsi-a3"));

        // list_co_mentioned_entities for tgsi-tool-tokio should include tgsi-tool-rust
        // (both are mentioned in a1, so they co-occur)
        let co = s.list_co_mentioned_entities("tgsi-tool-tokio").await.unwrap();
        assert!(!co.is_empty());
    }

    /// End-to-end test of P4 GraphSearcher driving real entity matching,
    /// MENTIONS traversal, and ENTITY_OVERLAP one-hop expansion.
    #[tokio::test]
    async fn graph_searcher_end_to_end() {
        use crate::retrieval::GraphSearcher;
        use crate::config::RetrievalConfig;

        let s = fixture().await;
        let ts = now();

        // Create three articles
        s.create_article(&Article {
            id: "gse-a1".into(), store_id: "s1".into(),
            title: "Rust Async Programming".into(),
            content: "Rust provides powerful async capabilities using Tokio runtime".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "gse-h1".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_article(&Article {
            id: "gse-a2".into(), store_id: "s1".into(),
            title: "Go Concurrency".into(),
            content: "Go uses goroutines for concurrent programming".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "gse-h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_article(&Article {
            id: "gse-a3".into(), store_id: "s1".into(),
            title: "Tokio Internals".into(),
            content: "Deep dive into how Tokio scheduler works".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "gse-h3".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        // Create entities
        s.create_entity(&Entity {
            id: "gse-tool-rust".into(), name: "Rust".into(), entity_type: "tool".into(),
            description: Some("Systems programming language".into()), store_id: "s1".into(),
            mention_count: 2, created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_entity(&Entity {
            id: "gse-tool-tokio".into(), name: "Tokio".into(), entity_type: "tool".into(),
            description: Some("Async runtime for Rust".into()), store_id: "s1".into(),
            mention_count: 2, created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        // MENTIONS edges
        s.create_mentions_edge("gse-a1", "gse-tool-rust", "Rust provides", 0.95).await.unwrap();
        s.create_mentions_edge("gse-a1", "gse-tool-tokio", "using Tokio", 0.90).await.unwrap();
        s.create_mentions_edge("gse-a3", "gse-tool-tokio", "Tokio scheduler", 0.92).await.unwrap();

        // ENTITY_OVERLAP (P5-renamed from RELATED_TO) — seeded directly into the new table.
        // Uses LET-binding for hyphenated IDs (SurrealDB 2 parses bare table:id-with-hyphen
        // as subtraction).
        s.db().query(r#"
            LET $from = type::thing('article', 'gse-a1');
            LET $to = type::thing('article', 'gse-a3');
            RELATE $from->entity_overlap->$to CONTENT {
                shared_entity_count: 1, strength: 0.5, confidence: 0.5,
                extraction_method: "heuristic", store_id: "s1",
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z"
            };
        "#).await.expect("seed overlap").check().expect("seed check");

        // Drive GraphSearcher end-to-end
        let config = RetrievalConfig::default();
        let db: std::sync::Arc<dyn Store> = std::sync::Arc::new(s);
        let searcher = GraphSearcher::new(db, config);

        // Query "Rust" → finds gse-a1 (direct mention) + gse-a3 (one-hop via overlap)
        let output = searcher.search("Rust", "s1", 10).await.expect("search Rust");
        assert!(!output.results.is_empty(), "Rust query must return results");
        assert!(output.entity_coverage > 0.0);
        let ids: Vec<&str> = output.results.iter().map(|r| r.article_id.as_str()).collect();
        assert!(ids.contains(&"gse-a1"), "direct Rust mention missing");
        // gse-a3 may appear via one-hop ENTITY_OVERLAP expansion if graph_hops >= 1
        // (default is 1). This verifies the P5 list_related_articles fix from Task 9
        // actually works end-to-end.
        assert!(ids.contains(&"gse-a3"), "one-hop expansion via entity_overlap failed — \
            P5 Task 9's list_related_articles retarget may be broken in production path");

        // Query "Tokio" → both gse-a1 and gse-a3 mention Tokio
        let output = searcher.search("Tokio", "s1", 10).await.expect("search Tokio");
        assert!(output.results.len() >= 2);
        let ids: Vec<&str> = output.results.iter().map(|r| r.article_id.as_str()).collect();
        assert!(ids.contains(&"gse-a1"));
        assert!(ids.contains(&"gse-a3"));

        // Query "Go" → no Go entity, no results, zero coverage
        let output = searcher.search("Go", "s1", 10).await.expect("search Go");
        assert!(output.results.is_empty());
        assert_eq!(output.entity_coverage, 0.0);
    }

    /// End-to-end test of P6 ActivationEngine driving intent classification,
    /// seed extraction, PPR diffusion over a typed multi-graph, and SYNAPSE
    /// post-processing.
    ///
    /// Fixture: 4 articles in store ae-s1. Two share the "outage" entity
    /// (a1, a2); a2 is causally linked to a1 via CAUSED_BY. A Why query
    /// should classify as Intent::Why, boost the CAUSED_BY edge (×4.0 per
    /// MAGMA Table 6), and produce non-empty results.
    #[tokio::test]
    async fn activation_engine_returns_results_for_why_query() {
        use crate::retrieval::{ActivationEngine, intent::Intent};
        use crate::config::RetrievalConfig;
        use std::sync::Arc;

        let s = fixture().await;
        let ts = now();

        // 4 articles
        for (id, title, content) in &[
            ("ae-a1", "Outage retrospective", "an outage occurred yesterday"),
            ("ae-a2", "Deploy that caused outage", "the deploy pushed a bad release"),
            ("ae-a3", "Unrelated article", "talking about gardening"),
            ("ae-a4", "Another unrelated", "completely different topic"),
        ] {
            s.create_article(&Article {
                id: id.to_string(), store_id: "ae-s1".into(),
                title: title.to_string(), content: content.to_string(),
                source_type: "user".into(), source_id: String::new(),
                content_hash: format!("{}-h", id), tags: serde_json::json!([]),
                embedded_at: None,
                created_at: ts.clone(), updated_at: ts.clone(),
            }).await.unwrap();
        }

        // Entities: "outage" mentioned in a1 and a2; "deploy" only in a2
        s.create_entity(&Entity {
            id: "ae-ent-outage".into(), name: "outage".into(),
            entity_type: "concept".into(), description: None,
            store_id: "ae-s1".into(), mention_count: 2,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_entity(&Entity {
            id: "ae-ent-deploy".into(), name: "deploy".into(),
            entity_type: "concept".into(), description: None,
            store_id: "ae-s1".into(), mention_count: 1,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        // MENTIONS edges
        s.create_mentions_edge("ae-a1", "ae-ent-outage", "outage retro", 0.95).await.unwrap();
        s.create_mentions_edge("ae-a2", "ae-ent-outage", "the outage", 0.92).await.unwrap();
        s.create_mentions_edge("ae-a2", "ae-ent-deploy", "deploy pushed", 0.95).await.unwrap();

        // ENTITY_OVERLAP: a1 and a2 share the "outage" entity
        s.db().query(r#"
            LET $from = type::thing('article', 'ae-a1');
            LET $to = type::thing('article', 'ae-a2');
            RELATE $from->entity_overlap->$to CONTENT {
                shared_entity_count: 1, strength: 0.5, confidence: 0.5,
                extraction_method: "heuristic", store_id: "ae-s1",
                created_at: "2026-05-24T00:00:00Z", updated_at: "2026-05-24T00:00:00Z"
            };
        "#).await.expect("seed overlap").check().expect("seed check");

        // CAUSED_BY: a2 (deploy article) caused a1 (outage)
        s.create_caused_by_edge(
            "ae-s1", "ae-a2", "ae-a1",
            0.9, Some("explicit causal chain".into())
        ).await.unwrap();

        // Build the engine with caused_by + entity_overlap enabled
        let mut config = RetrievalConfig::default();
        config.edge_types.caused_by = true;
        config.edge_types.entity_overlap = true;

        let db: Arc<dyn Store> = Arc::new(s);
        let engine = ActivationEngine::new(db, config);

        // Query mentioning "outage" — entity match → seeds = a1, a2
        // Should classify as Intent::Why due to "why" cue
        let output = engine.search("why did the outage happen?", "ae-s1", 10).await.unwrap();

        assert_eq!(output.intent, Intent::Why,
            "query with 'why' should classify as Why intent");
        assert!(!output.results.is_empty(),
            "engine should return at least one result; got 0");

        // At least one result must be from the outage-linked subgraph (a1 or a2)
        let ids: Vec<&str> = output.results.iter().map(|r| r.article_id.as_str()).collect();
        let has_outage_article = ids.iter().any(|id| *id == "ae-a1" || *id == "ae-a2");
        assert!(has_outage_article,
            "expected at least one outage-linked article in results; got {:?}", ids);

        // node_count should be >= 2 (a1 + a2 from seed expansion)
        assert!(output.node_count >= 2,
            "subgraph should include at least the two seed articles; got {}", output.node_count);

        // Verify K2KResult metadata is populated
        if let Some(first) = output.results.first() {
            let meta = &first.metadata;
            assert_eq!(meta.get("search_type").and_then(|v| v.as_str()), Some("activation"));
            assert!(meta.get("intent").is_some(), "intent should be in metadata");
            assert!(meta.get("activation_score").is_some(), "activation_score should be in metadata");
        }
    }

    /// Verify the engine handles queries with no entity matches gracefully —
    /// returns empty results with zero coverage, not an error.
    #[tokio::test]
    async fn activation_engine_empty_for_no_entity_matches() {
        use crate::retrieval::ActivationEngine;
        use crate::config::RetrievalConfig;
        use std::sync::Arc;

        let s = fixture().await;
        let ts = now();

        s.create_article(&Article {
            id: "ae2-a1".into(), store_id: "ae2-s1".into(),
            title: "Some article".into(), content: "content".into(),
            source_type: "user".into(), source_id: String::new(),
            content_hash: "ae2-h1".into(), tags: serde_json::json!([]),
            embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        let db: Arc<dyn Store> = Arc::new(s);
        let engine = ActivationEngine::new(db, RetrievalConfig::default());

        // No entities seeded; any query produces empty seeds → empty results
        let output = engine.search("nonsense query xyzzy", "ae2-s1", 10).await.unwrap();

        assert!(output.results.is_empty(), "no matched entities should yield empty results");
        assert_eq!(output.entity_coverage, 0.0);
        assert_eq!(output.node_count, 0);
    }
}
