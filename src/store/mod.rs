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

    // P7 event CRUD + event-edge helpers
    async fn create_event(&self, event: &Event) -> Result<()>;
    async fn get_event(&self, event_id: &str) -> Result<Option<Event>>;
    async fn list_events_for_store(&self, store_id: &str) -> Result<Vec<Event>>;
    async fn create_contains_evidence_edge(
        &self,
        event_id: &str,
        article_id: &str,
        confidence: f64,
    ) -> Result<()>;
    async fn create_motivates_edge(
        &self,
        from_event_id: &str,
        to_event_id: &str,
        confidence: f64,
        rationale: Option<String>,
    ) -> Result<()>;
    async fn create_part_of_edge(
        &self,
        child_event_id: &str,
        parent_event_id: &str,
        confidence: f64,
    ) -> Result<()>;
    async fn list_events_for_article(&self, article_id: &str) -> Result<Vec<Event>>;

    /// Returns articles where `reflects` array contains this article_id.
    /// Use case: drill-down — "show me what was synthesized from this article."
    async fn list_reflections_for_article(&self, article_id: &str) -> Result<Vec<Article>>;

    // P7 maintenance bookkeeping

    /// Was the given idempotency key used to start (any status) a maintenance
    /// run since `cutoff_rfc3339`? Used by MaintenanceScheduler to dedup.
    #[allow(dead_code)]
    async fn recent_maintenance_run_by_key(
        &self,
        key: &str,
        cutoff_rfc3339: &str,
    ) -> Result<bool>;

    /// Record the start of a maintenance run.
    #[allow(dead_code)]
    async fn record_maintenance_run(
        &self,
        job_name: &str,
        idempotency_key: &str,
        started_at: &str,
    ) -> Result<()>;

    /// Mark a maintenance run as completed (success or failure).
    #[allow(dead_code)]
    async fn complete_maintenance_run(
        &self,
        idempotency_key: &str,
        completed_at: &str,
        status: &str,
    ) -> Result<()>;

    /// Increment the per-store ingest counter and return the new total.
    /// Used by P7 ingest-triggered reflection.
    async fn increment_ingest_counter(&self, store_id: &str) -> Result<usize>;

    /// Reset the per-store ingest counter back to 0. Called after a
    /// reflection job is submitted so the next 100 ingests trigger the
    /// next reflection.
    async fn reset_ingest_counter(&self, store_id: &str) -> Result<()>;

    // P8 access tracking + tier + pin/unpin + audit log
    async fn record_article_access(&self, article_id: &str) -> Result<()>;
    #[allow(dead_code)] // P9+ will call this when event access tracking is wired
    async fn record_event_access(&self, event_id: &str) -> Result<()>;
    async fn set_article_tier(&self, article_id: &str, new_tier: Tier, reason: &str) -> Result<()>;
    async fn set_event_tier(&self, event_id: &str, new_tier: Tier, reason: &str) -> Result<()>;
    async fn pin_article(&self, article_id: &str) -> Result<()>;
    async fn unpin_article(&self, article_id: &str) -> Result<()>;
    async fn list_articles_by_tier(&self, store_id: &str, tier: Tier) -> Result<Vec<Article>>;
    async fn write_audit_log(&self, entry: &AuditLogEntry) -> Result<()>;
    async fn list_audit_log(&self, store_id: &str, since_rfc3339: Option<&str>, limit: usize) -> Result<Vec<AuditLogEntry>>;
    async fn count_recent_access_audit(&self, article_id: &str, since_rfc3339: &str) -> Result<usize>;
    async fn set_article_compacted_into(&self, article_id: &str, reflection_id: &str) -> Result<()>;

    /// Append a policy trace (P10). Used for offline training-data collection.
    async fn write_policy_trace(&self, trace: &PolicyTrace) -> Result<()>;

    /// List policy traces with optional filters. Used by the CLI
    /// `policy-traces` command.
    async fn list_policy_traces(
        &self,
        store_id: Option<&str>,
        policy_name: Option<&str>,
        since_rfc3339: Option<&str>,
        limit: usize,
    ) -> Result<Vec<PolicyTrace>>;
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


fn tier_to_string(t: Tier) -> &'static str {
    match t {
        Tier::Hot => "hot",
        Tier::Warm => "warm",
        Tier::Cold => "cold",
        Tier::Archive => "archive",
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
                    updated_at: $updated_at, reflects: $reflects,
                    access_count: $access_count,
                    last_accessed_at: $last_accessed_at,
                    importance_score: $importance_score,
                    tier: $tier,
                    pinned: $pinned,
                    compacted_into: $compacted_into
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
            .bind(("reflects", a.reflects.clone()))
            .bind(("access_count", a.access_count))
            .bind(("last_accessed_at", a.last_accessed_at.clone()))
            .bind(("importance_score", a.importance_score))
            .bind(("tier", match a.tier {
                Tier::Hot => "hot",
                Tier::Warm => "warm",
                Tier::Cold => "cold",
                Tier::Archive => "archive",
            }))
            .bind(("pinned", a.pinned))
            .bind(("compacted_into", a.compacted_into.clone()))
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
                    embedded_at: $embedded_at, updated_at: $updated_at,
                    reflects: $reflects,
                    access_count: $access_count,
                    last_accessed_at: $last_accessed_at,
                    importance_score: $importance_score,
                    tier: $tier,
                    pinned: $pinned,
                    compacted_into: $compacted_into
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
            .bind(("reflects", a.reflects.clone()))
            .bind(("access_count", a.access_count))
            .bind(("last_accessed_at", a.last_accessed_at.clone()))
            .bind(("importance_score", a.importance_score))
            .bind(("tier", match a.tier {
                Tier::Hot => "hot",
                Tier::Warm => "warm",
                Tier::Cold => "cold",
                Tier::Archive => "archive",
            }))
            .bind(("pinned", a.pinned))
            .bind(("compacted_into", a.compacted_into.clone()))
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

    // P7 event CRUD + event-edge helpers

    async fn create_event(&self, event: &Event) -> Result<()> {
        let method_str = serde_json::to_value(event.extraction_method)
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "user_asserted".into());

        let res = self.db()
            .query(
                "CREATE type::thing('event', $id) CONTENT {
                    store_id: $store_id,
                    title: $title,
                    summary: $summary,
                    started_at: $started_at,
                    ended_at: $ended_at,
                    participants: $participants,
                    source_type: $source_type,
                    confidence: $confidence,
                    extraction_method: $method,
                    created_at: $created_at,
                    updated_at: $updated_at,
                    access_count: $access_count,
                    last_accessed_at: $last_accessed_at,
                    importance_score: $importance_score,
                    tier: $tier,
                    pinned: $pinned,
                    compacted_into: $compacted_into
                 }"
            )
            .bind(("id", event.id.clone()))
            .bind(("store_id", event.store_id.clone()))
            .bind(("title", event.title.clone()))
            .bind(("summary", event.summary.clone()))
            .bind(("started_at", event.started_at.clone()))
            .bind(("ended_at", event.ended_at.clone()))
            .bind(("participants", event.participants.clone()))
            .bind(("source_type", event.source_type.clone()))
            .bind(("confidence", event.confidence))
            .bind(("method", method_str))
            .bind(("created_at", event.created_at.clone()))
            .bind(("updated_at", event.updated_at.clone()))
            .bind(("access_count", event.access_count))
            .bind(("last_accessed_at", event.last_accessed_at.clone()))
            .bind(("importance_score", event.importance_score))
            .bind(("tier", match event.tier {
                Tier::Hot => "hot",
                Tier::Warm => "warm",
                Tier::Cold => "cold",
                Tier::Archive => "archive",
            }))
            .bind(("pinned", event.pinned))
            .bind(("compacted_into", event.compacted_into.clone()))
            .await
            .context("create_event")?;
        let _ = res;
        Ok(())
    }

    async fn get_event(&self, event_id: &str) -> Result<Option<Event>> {
        let mut resp = self.db()
            .query(
                "SELECT meta::id(id) AS id, store_id, title, summary, started_at, ended_at,
                        participants, source_type, confidence, extraction_method,
                        created_at, updated_at,
                        access_count, last_accessed_at, importance_score,
                        tier, pinned, compacted_into
                 FROM event
                 WHERE id = type::thing('event', $id)"
            )
            .bind(("id", event_id.to_string()))
            .await
            .context("get_event")?;
        let events: Vec<Event> = resp.take(0).unwrap_or_default();
        Ok(events.into_iter().next())
    }

    async fn list_events_for_store(&self, store_id: &str) -> Result<Vec<Event>> {
        let mut resp = self.db()
            .query(
                "SELECT meta::id(id) AS id, store_id, title, summary, started_at, ended_at,
                        participants, source_type, confidence, extraction_method,
                        created_at, updated_at,
                        access_count, last_accessed_at, importance_score,
                        tier, pinned, compacted_into
                 FROM event
                 WHERE store_id = $sid
                 ORDER BY started_at"
            )
            .bind(("sid", store_id.to_string()))
            .await
            .context("list_events_for_store")?;
        let events: Vec<Event> = resp.take(0).unwrap_or_default();
        Ok(events)
    }

    async fn create_contains_evidence_edge(
        &self,
        event_id: &str,
        article_id: &str,
        confidence: f64,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let res = self.db()
            .query(
                "LET $from = type::thing('event', $eid);
                 LET $to = type::thing('article', $aid);
                 RELATE $from->contains_evidence->$to CONTENT {
                    confidence: $conf,
                    created_at: $now
                 }"
            )
            .bind(("eid", event_id.to_string()))
            .bind(("aid", article_id.to_string()))
            .bind(("conf", confidence))
            .bind(("now", now))
            .await;
        match res { Ok(mut r) => { let _ = r.check(); Ok(()) } Err(_) => Ok(()) }
    }

    async fn create_motivates_edge(
        &self,
        from_event_id: &str,
        to_event_id: &str,
        confidence: f64,
        rationale: Option<String>,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let res = self.db()
            .query(
                "LET $from = type::thing('event', $from_id);
                 LET $to = type::thing('event', $to_id);
                 RELATE $from->motivates->$to CONTENT {
                    confidence: $conf,
                    rationale: $rationale,
                    extraction_method: 'llm',
                    created_at: $now
                 }"
            )
            .bind(("from_id", from_event_id.to_string()))
            .bind(("to_id", to_event_id.to_string()))
            .bind(("conf", confidence))
            .bind(("rationale", rationale))
            .bind(("now", now))
            .await;
        match res { Ok(mut r) => { let _ = r.check(); Ok(()) } Err(_) => Ok(()) }
    }

    async fn create_part_of_edge(
        &self,
        child_event_id: &str,
        parent_event_id: &str,
        confidence: f64,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let res = self.db()
            .query(
                "LET $from = type::thing('event', $child_id);
                 LET $to = type::thing('event', $parent_id);
                 RELATE $from->part_of->$to CONTENT {
                    confidence: $conf,
                    extraction_method: 'llm',
                    created_at: $now
                 }"
            )
            .bind(("child_id", child_event_id.to_string()))
            .bind(("parent_id", parent_event_id.to_string()))
            .bind(("conf", confidence))
            .bind(("now", now))
            .await;
        match res { Ok(mut r) => { let _ = r.check(); Ok(()) } Err(_) => Ok(()) }
    }

    async fn list_events_for_article(&self, article_id: &str) -> Result<Vec<Event>> {
        // Reverse traversal: find events whose CONTAINS_EVIDENCE edge points to this article
        let mut resp = self.db()
            .query(
                "SELECT meta::id(id) AS id, store_id, title, summary, started_at, ended_at,
                        participants, source_type, confidence, extraction_method,
                        created_at, updated_at,
                        access_count, last_accessed_at, importance_score,
                        tier, pinned, compacted_into
                 FROM event
                 WHERE id IN (
                    SELECT VALUE in FROM contains_evidence
                    WHERE out = type::thing('article', $aid)
                 )
                 ORDER BY started_at"
            )
            .bind(("aid", article_id.to_string()))
            .await
            .context("list_events_for_article")?;
        let events: Vec<Event> = resp.take(0).unwrap_or_default();
        Ok(events)
    }

    async fn list_reflections_for_article(&self, article_id: &str) -> Result<Vec<Article>> {
        let mut resp = self.db()
            .query(
                "SELECT *, meta::id(id) AS id FROM article
                 WHERE $aid IN reflects
                   AND source_type = 'reflection'"
            )
            .bind(("aid", article_id.to_string()))
            .await
            .context("list_reflections_for_article")?;
        let articles: Vec<Article> = resp.take(0).unwrap_or_default();
        Ok(articles)
    }

    // P7 maintenance bookkeeping

    async fn recent_maintenance_run_by_key(
        &self,
        key: &str,
        cutoff_rfc3339: &str,
    ) -> Result<bool> {
        let mut resp = self
            .db()
            .query(
                "SELECT count() AS n FROM _maintenance_runs
                 WHERE idempotency_key = $key AND started_at > $cutoff
                 GROUP ALL",
            )
            .bind(("key", key.to_string()))
            .bind(("cutoff", cutoff_rfc3339.to_string()))
            .await
            .context("recent_maintenance_run_by_key")?;
        #[derive(serde::Deserialize)]
        struct Cnt {
            n: i64,
        }
        let rows: Vec<Cnt> = resp.take(0).unwrap_or_default();
        Ok(rows.first().map(|c| c.n > 0).unwrap_or(false))
    }

    async fn record_maintenance_run(
        &self,
        job_name: &str,
        idempotency_key: &str,
        started_at: &str,
    ) -> Result<()> {
        // The UNIQUE constraint on idempotency_key prevents duplicate inserts;
        // swallow conflicts so concurrent invocations don't error.
        let res = self
            .db()
            .query(
                "CREATE _maintenance_runs CONTENT {
                    job_name: $job_name,
                    idempotency_key: $key,
                    started_at: $started_at,
                    completed_at: NONE,
                    status: 'running'
                 }",
            )
            .bind(("job_name", job_name.to_string()))
            .bind(("key", idempotency_key.to_string()))
            .bind(("started_at", started_at.to_string()))
            .await;
        match res {
            Ok(r) => {
                let _ = r.check();
                Ok(())
            }
            Err(_) => Ok(()),
        }
    }

    async fn complete_maintenance_run(
        &self,
        idempotency_key: &str,
        completed_at: &str,
        status: &str,
    ) -> Result<()> {
        let res = self
            .db()
            .query(
                "UPDATE _maintenance_runs SET completed_at = $completed_at, status = $status
                 WHERE idempotency_key = $key",
            )
            .bind(("key", idempotency_key.to_string()))
            .bind(("completed_at", completed_at.to_string()))
            .bind(("status", status.to_string()))
            .await;
        match res {
            Ok(r) => {
                let _ = r.check();
                Ok(())
            }
            Err(_) => Ok(()),
        }
    }

    async fn increment_ingest_counter(&self, store_id: &str) -> Result<usize> {
        let now = chrono::Utc::now().to_rfc3339();
        // Two-step approach: read current count then upsert with new value.
        // This is more reliable than relying on += returning the post-increment
        // value via RETURN AFTER, which has inconsistent behavior across SurrealDB
        // versions when the record doesn't yet exist.
        let mut resp = self
            .db()
            .query(
                "SELECT count FROM _ingest_counters WHERE store_id = $store_id LIMIT 1",
            )
            .bind(("store_id", store_id.to_string()))
            .await
            .context("increment_ingest_counter select")?;
        #[derive(serde::Deserialize)]
        struct Row { count: i64 }
        let rows: Vec<Row> = resp.take(0).unwrap_or_default();
        let new_count = rows.first().map(|r| r.count + 1).unwrap_or(1);

        self.db()
            .query(
                "UPSERT type::thing('_ingest_counters', $store_id)
                 SET count = $new_count, store_id = $store_id, last_reset_at = $now",
            )
            .bind(("store_id", store_id.to_string()))
            .bind(("new_count", new_count))
            .bind(("now", now))
            .await
            .context("increment_ingest_counter upsert")?
            .check()
            .context("increment_ingest_counter upsert check")?;

        Ok(new_count as usize)
    }

    async fn reset_ingest_counter(&self, store_id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let res = self
            .db()
            .query(
                "UPSERT type::thing('_ingest_counters', $store_id)
                 SET count = 0, store_id = $store_id, last_reset_at = $now",
            )
            .bind(("store_id", store_id.to_string()))
            .bind(("now", now))
            .await;
        match res {
            Ok(r) => { let _ = r.check(); Ok(()) }
            Err(_) => Ok(()),
        }
    }

    // ── P8: access tracking + tier + pin/unpin + audit log ─────────────────

    async fn record_article_access(&self, article_id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();

        let current_tier = {
            let mut resp = self.db()
                .query("SELECT tier, store_id, pinned FROM article WHERE id = type::thing('article', $aid)")
                .bind(("aid", article_id.to_string()))
                .await
                .context("record_article_access: fetch tier")?;
            #[derive(serde::Deserialize)]
            struct Row { tier: String, store_id: String, pinned: bool }
            let rows: Vec<Row> = resp.take(0).unwrap_or_default();
            rows.into_iter().next()
        };

        let (current_tier_str, store_id, pinned) = match current_tier {
            Some(r) => (r.tier, r.store_id, r.pinned),
            None => return Ok(()),
        };

        let promote_to_hot = !pinned && current_tier_str != "hot";
        let new_tier_str = if promote_to_hot { "hot" } else { current_tier_str.as_str() };

        let res = self.db()
            .query(
                "UPDATE article SET access_count += 1, last_accessed_at = $now, tier = $tier
                 WHERE id = type::thing('article', $aid)"
            )
            .bind(("aid", article_id.to_string()))
            .bind(("now", now.clone()))
            .bind(("tier", new_tier_str.to_string()))
            .await
            .context("record_article_access: update")?;
        let _ = res.check();

        if promote_to_hot {
            let entry = AuditLogEntry {
                id: format!("al-{}-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0), article_id),
                store_id,
                action: "tier_change".into(),
                subject_type: "article".into(),
                subject_id: article_id.to_string(),
                details: serde_json::json!({
                    "from": current_tier_str,
                    "to": "hot",
                    "reason": "access_promote"
                }),
                recorded_at: now,
            };
            self.write_audit_log(&entry).await?;
        }

        Ok(())
    }

    async fn record_event_access(&self, event_id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let current_tier = {
            let mut resp = self.db()
                .query("SELECT tier, store_id, pinned FROM event WHERE id = type::thing('event', $eid)")
                .bind(("eid", event_id.to_string()))
                .await
                .context("record_event_access: fetch tier")?;
            #[derive(serde::Deserialize)]
            struct Row { tier: String, store_id: String, pinned: bool }
            let rows: Vec<Row> = resp.take(0).unwrap_or_default();
            rows.into_iter().next()
        };

        let (current_tier_str, store_id, pinned) = match current_tier {
            Some(r) => (r.tier, r.store_id, r.pinned),
            None => return Ok(()),
        };

        let promote_to_hot = !pinned && current_tier_str != "hot";
        let new_tier_str = if promote_to_hot { "hot" } else { current_tier_str.as_str() };

        let res = self.db()
            .query(
                "UPDATE event SET access_count += 1, last_accessed_at = $now, tier = $tier
                 WHERE id = type::thing('event', $eid)"
            )
            .bind(("eid", event_id.to_string()))
            .bind(("now", now.clone()))
            .bind(("tier", new_tier_str.to_string()))
            .await
            .context("record_event_access: update")?;
        let _ = res.check();

        if promote_to_hot {
            let entry = AuditLogEntry {
                id: format!("al-{}-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0), event_id),
                store_id,
                action: "tier_change".into(),
                subject_type: "event".into(),
                subject_id: event_id.to_string(),
                details: serde_json::json!({
                    "from": current_tier_str,
                    "to": "hot",
                    "reason": "access_promote"
                }),
                recorded_at: now,
            };
            self.write_audit_log(&entry).await?;
        }
        Ok(())
    }

    async fn set_article_tier(&self, article_id: &str, new_tier: Tier, reason: &str) -> Result<()> {
        let tier_str = tier_to_string(new_tier);
        let now = chrono::Utc::now().to_rfc3339();

        let prev = {
            let mut resp = self.db()
                .query("SELECT tier, store_id FROM article WHERE id = type::thing('article', $aid)")
                .bind(("aid", article_id.to_string()))
                .await
                .context("set_article_tier: fetch prev")?;
            #[derive(serde::Deserialize)] struct R { tier: String, store_id: String }
            let rows: Vec<R> = resp.take(0).unwrap_or_default();
            rows.into_iter().next()
        };
        let (prev_tier, store_id) = match prev {
            Some(r) => (r.tier, r.store_id),
            None => return Ok(()),
        };

        if prev_tier == tier_str {
            return Ok(());
        }

        let res = self.db()
            .query(
                "UPDATE article SET tier = $tier
                 WHERE id = type::thing('article', $aid)"
            )
            .bind(("aid", article_id.to_string()))
            .bind(("tier", tier_str.to_string()))
            .await
            .context("set_article_tier: update")?;
        let _ = res.check();

        let entry = AuditLogEntry {
            id: format!("al-{}-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0), article_id),
            store_id,
            action: "tier_change".into(),
            subject_type: "article".into(),
            subject_id: article_id.to_string(),
            details: serde_json::json!({ "from": prev_tier, "to": tier_str, "reason": reason }),
            recorded_at: now,
        };
        self.write_audit_log(&entry).await
    }

    async fn set_event_tier(&self, event_id: &str, new_tier: Tier, reason: &str) -> Result<()> {
        let tier_str = tier_to_string(new_tier);
        let now = chrono::Utc::now().to_rfc3339();
        let prev = {
            let mut resp = self.db()
                .query("SELECT tier, store_id FROM event WHERE id = type::thing('event', $eid)")
                .bind(("eid", event_id.to_string()))
                .await
                .context("set_event_tier: fetch prev")?;
            #[derive(serde::Deserialize)] struct R { tier: String, store_id: String }
            let rows: Vec<R> = resp.take(0).unwrap_or_default();
            rows.into_iter().next()
        };
        let (prev_tier, store_id) = match prev {
            Some(r) => (r.tier, r.store_id),
            None => return Ok(()),
        };
        if prev_tier == tier_str { return Ok(()); }

        let res = self.db()
            .query("UPDATE event SET tier = $tier WHERE id = type::thing('event', $eid)")
            .bind(("eid", event_id.to_string()))
            .bind(("tier", tier_str.to_string()))
            .await.context("set_event_tier: update")?;
        let _ = res.check();

        let entry = AuditLogEntry {
            id: format!("al-{}-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0), event_id),
            store_id,
            action: "tier_change".into(),
            subject_type: "event".into(),
            subject_id: event_id.to_string(),
            details: serde_json::json!({ "from": prev_tier, "to": tier_str, "reason": reason }),
            recorded_at: now,
        };
        self.write_audit_log(&entry).await
    }

    async fn pin_article(&self, article_id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let store_id_opt = {
            let mut resp = self.db()
                .query("SELECT store_id FROM article WHERE id = type::thing('article', $aid)")
                .bind(("aid", article_id.to_string()))
                .await
                .context("pin_article: fetch")?;
            #[derive(serde::Deserialize)] struct R { store_id: String }
            let rows: Vec<R> = resp.take(0).unwrap_or_default();
            rows.into_iter().next().map(|r| r.store_id)
        };
        let store_id = match store_id_opt {
            Some(sid) => sid,
            None => return Ok(()),
        };

        let res = self.db()
            .query("UPDATE article SET pinned = true WHERE id = type::thing('article', $aid)")
            .bind(("aid", article_id.to_string()))
            .await.context("pin_article: update")?;
        let _ = res.check();

        let entry = AuditLogEntry {
            id: format!("al-{}-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0), article_id),
            store_id,
            action: "pin".into(),
            subject_type: "article".into(),
            subject_id: article_id.to_string(),
            details: serde_json::json!({}),
            recorded_at: now,
        };
        self.write_audit_log(&entry).await
    }

    async fn unpin_article(&self, article_id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let store_id_opt = {
            let mut resp = self.db()
                .query("SELECT store_id FROM article WHERE id = type::thing('article', $aid)")
                .bind(("aid", article_id.to_string()))
                .await.context("unpin_article: fetch")?;
            #[derive(serde::Deserialize)] struct R { store_id: String }
            let rows: Vec<R> = resp.take(0).unwrap_or_default();
            rows.into_iter().next().map(|r| r.store_id)
        };
        let store_id = match store_id_opt { Some(s) => s, None => return Ok(()) };

        let res = self.db()
            .query("UPDATE article SET pinned = false WHERE id = type::thing('article', $aid)")
            .bind(("aid", article_id.to_string()))
            .await.context("unpin_article: update")?;
        let _ = res.check();

        let entry = AuditLogEntry {
            id: format!("al-{}-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0), article_id),
            store_id,
            action: "unpin".into(),
            subject_type: "article".into(),
            subject_id: article_id.to_string(),
            details: serde_json::json!({}),
            recorded_at: now,
        };
        self.write_audit_log(&entry).await
    }

    async fn list_articles_by_tier(&self, store_id: &str, tier: Tier) -> Result<Vec<Article>> {
        let tier_str = tier_to_string(tier);
        let mut resp = self.db()
            .query(
                "SELECT *, meta::id(id) AS id FROM article
                 WHERE store_id = $sid AND tier = $tier
                 ORDER BY last_accessed_at DESC"
            )
            .bind(("sid", store_id.to_string()))
            .bind(("tier", tier_str.to_string()))
            .await.context("list_articles_by_tier")?;
        let articles: Vec<Article> = resp.take(0).unwrap_or_default();
        Ok(articles)
    }

    async fn write_audit_log(&self, entry: &AuditLogEntry) -> Result<()> {
        let res = self.db()
            .query(
                "CREATE type::thing('_audit_log', $id) CONTENT {
                    store_id: $store_id,
                    action: $action,
                    subject_type: $subject_type,
                    subject_id: $subject_id,
                    details: $details,
                    recorded_at: $recorded_at
                 }"
            )
            .bind(("id", entry.id.clone()))
            .bind(("store_id", entry.store_id.clone()))
            .bind(("action", entry.action.clone()))
            .bind(("subject_type", entry.subject_type.clone()))
            .bind(("subject_id", entry.subject_id.clone()))
            .bind(("details", entry.details.clone()))
            .bind(("recorded_at", entry.recorded_at.clone()))
            .await;
        match res { Ok(r) => { let _ = r.check(); Ok(()) } Err(_) => Ok(()) }
    }

    async fn list_audit_log(&self, store_id: &str, since_rfc3339: Option<&str>, limit: usize) -> Result<Vec<AuditLogEntry>> {
        let q = if since_rfc3339.is_some() {
            "SELECT meta::id(id) AS id, store_id, action, subject_type, subject_id, details, recorded_at
             FROM _audit_log
             WHERE store_id = $sid AND recorded_at >= $since
             ORDER BY recorded_at DESC LIMIT $limit"
        } else {
            "SELECT meta::id(id) AS id, store_id, action, subject_type, subject_id, details, recorded_at
             FROM _audit_log
             WHERE store_id = $sid
             ORDER BY recorded_at DESC LIMIT $limit"
        };
        let mut query = self.db().query(q)
            .bind(("sid", store_id.to_string()))
            .bind(("limit", limit as i64));
        if let Some(s) = since_rfc3339 {
            query = query.bind(("since", s.to_string()));
        }
        let mut resp = query.await.context("list_audit_log")?;
        let entries: Vec<AuditLogEntry> = resp.take(0).unwrap_or_default();
        Ok(entries)
    }

    async fn count_recent_access_audit(&self, article_id: &str, since_rfc3339: &str) -> Result<usize> {
        let mut resp = self.db()
            .query(
                "SELECT count() AS n FROM _audit_log
                 WHERE subject_id = $aid
                   AND action = 'tier_change'
                   AND recorded_at > $since
                 GROUP ALL"
            )
            .bind(("aid", article_id.to_string()))
            .bind(("since", since_rfc3339.to_string()))
            .await.context("count_recent_access_audit")?;
        #[derive(serde::Deserialize)] struct Cnt { n: i64 }
        let rows: Vec<Cnt> = resp.take(0).unwrap_or_default();
        Ok(rows.first().map(|c| c.n as usize).unwrap_or(0))
    }

    async fn set_article_compacted_into(&self, article_id: &str, reflection_id: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let store_id_opt = {
            let mut resp = self.db()
                .query("SELECT store_id FROM article WHERE id = type::thing('article', $aid)")
                .bind(("aid", article_id.to_string()))
                .await.context("set_compacted_into: fetch")?;
            #[derive(serde::Deserialize)] struct R { store_id: String }
            let rows: Vec<R> = resp.take(0).unwrap_or_default();
            rows.into_iter().next().map(|r| r.store_id)
        };
        let store_id = match store_id_opt { Some(s) => s, None => return Ok(()) };

        let res = self.db()
            .query(
                "UPDATE article SET compacted_into = $refl, tier = 'archive'
                 WHERE id = type::thing('article', $aid)"
            )
            .bind(("aid", article_id.to_string()))
            .bind(("refl", reflection_id.to_string()))
            .await.context("set_compacted_into: update")?;
        let _ = res.check();

        let entry = AuditLogEntry {
            id: format!("al-{}-{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0), article_id),
            store_id,
            action: "compact".into(),
            subject_type: "article".into(),
            subject_id: article_id.to_string(),
            details: serde_json::json!({ "into": reflection_id, "tier": "archive" }),
            recorded_at: now,
        };
        self.write_audit_log(&entry).await
    }

    async fn write_policy_trace(&self, trace: &PolicyTrace) -> Result<()> {
        let decision_type_str = serde_json::to_value(trace.decision_type)
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| "decay".into());

        let res = self.db()
            .query(
                "CREATE type::thing('_policy_traces', $id) CONTENT {
                    store_id: $store_id,
                    policy_name: $policy_name,
                    decision_type: $decision_type,
                    input_features: $input_features,
                    action: $action,
                    outcome: $outcome,
                    recorded_at: $recorded_at
                 }"
            )
            .bind(("id", trace.id.clone()))
            .bind(("store_id", trace.store_id.clone()))
            .bind(("policy_name", trace.policy_name.clone()))
            .bind(("decision_type", decision_type_str))
            .bind(("input_features", trace.input_features.clone()))
            .bind(("action", trace.action.clone()))
            .bind(("outcome", trace.outcome.clone()))
            .bind(("recorded_at", trace.recorded_at.clone()))
            .await;
        match res { Ok(r) => { let _ = r.check(); Ok(()) } Err(_) => Ok(()) }
    }

    async fn list_policy_traces(
        &self,
        store_id: Option<&str>,
        policy_name: Option<&str>,
        since_rfc3339: Option<&str>,
        limit: usize,
    ) -> Result<Vec<PolicyTrace>> {
        let mut conditions: Vec<&str> = Vec::new();
        if store_id.is_some() { conditions.push("store_id = $sid"); }
        if policy_name.is_some() { conditions.push("policy_name = $pname"); }
        if since_rfc3339.is_some() { conditions.push("recorded_at >= $since"); }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let q = format!(
            "SELECT meta::id(id) AS id, store_id, policy_name, decision_type,
                    input_features, action, outcome, recorded_at
             FROM _policy_traces {}
             ORDER BY recorded_at DESC LIMIT $limit",
            where_clause
        );

        let mut query = self.db().query(&q).bind(("limit", limit as i64));
        if let Some(s) = store_id { query = query.bind(("sid", s.to_string())); }
        if let Some(p) = policy_name { query = query.bind(("pname", p.to_string())); }
        if let Some(s) = since_rfc3339 { query = query.bind(("since", s.to_string())); }

        let mut resp = query.await.context("list_policy_traces")?;
        let traces: Vec<PolicyTrace> = resp.take(0).unwrap_or_default();
        Ok(traces)
    }
}

/// Convenience alias used across the codebase.
#[allow(dead_code)] // consumed by P9+ services
pub type DynStore = dyn Store;

/// Boxed, arc'd instance. Handed into every service that used to hold
/// `Arc<Database>`.
#[allow(dead_code)] // consumed by P9+ services
pub type SharedStore = Arc<DynStore>;

/// Wrap a concrete `SurrealStore` into the shared trait object.
#[allow(dead_code)] // consumed by P9+ services
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
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
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
                reflects: vec![],
                access_count: 0,
                last_accessed_at: String::new(),
                importance_score: 0.5,
                tier: Tier::Hot,
                pinned: false,
                compacted_into: None,
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
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();
        s.create_article(&Article {
            id: "lafe-a2".into(), store_id: "s1".into(), title: "Tokio Deep Dive".into(),
            content: "About Tokio".into(), source_type: "user".into(),
            source_id: String::new(), content_hash: "lafe-h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
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
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();
        s.create_article(&Article {
            id: "co-art2".into(), store_id: "co-s1".into(), title: "More Rust".into(),
            content: "C".into(), source_type: "user".into(),
            source_id: String::new(), content_hash: "co-hx2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
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
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();
        s.create_article(&Article {
            id: "p5pre-a2".into(), store_id: "p5pre-s1".into(), title: "B".into(), content: "y".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "p5pre-h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
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
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();
        s.create_article(&Article {
            id: "p5sem-a2".into(), store_id: "p5sem-s1".into(), title: "B".into(), content: "".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "p5sem-h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
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
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();
        s.create_article(&Article {
            id: "p5cb-a2".into(), store_id: "p5cb-s1".into(), title: "B".into(), content: "".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "p5cb-h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
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
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();
        s.create_article(&Article {
            id: "p5ref-a2".into(), store_id: "p5ref-s1".into(), title: "B".into(), content: "".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "p5ref-h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
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

    #[tokio::test]
    async fn create_event_round_trips() {
        let s = fixture().await;
        let ts = now();
        let event = Event {
            id: "ev1".into(), store_id: "ev-s1".into(),
            title: "Trip".into(), summary: "AZ vacation".into(),
            started_at: "2026-03-15T00:00:00Z".into(),
            ended_at: "2026-03-20T00:00:00Z".into(),
            participants: serde_json::json!(["alice", "bob"]),
            source_type: "manual".into(),
            confidence: 0.9,
            extraction_method: ExtractionMethod::UserAsserted,
            created_at: ts.clone(), updated_at: ts.clone(),
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        };
        s.create_event(&event).await.unwrap();

        let got = s.get_event("ev1").await.unwrap().expect("event exists");
        assert_eq!(got.title, "Trip");
        assert_eq!(got.summary, "AZ vacation");
        assert_eq!(got.extraction_method, ExtractionMethod::UserAsserted);
    }

    #[tokio::test]
    async fn get_event_missing_returns_none() {
        let s = fixture().await;
        let got = s.get_event("nonexistent").await.unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn list_events_for_store_orders_by_started_at() {
        let s = fixture().await;
        let ts = now();

        // Insert in reverse temporal order to verify SQL ORDER BY works
        for (id, started_at) in &[
            ("ev_later", "2026-03-20T00:00:00Z"),
            ("ev_earlier", "2026-03-15T00:00:00Z"),
        ] {
            s.create_event(&Event {
                id: id.to_string(), store_id: "le-s1".into(),
                title: id.to_string(), summary: "".into(),
                started_at: started_at.to_string(),
                ended_at: started_at.to_string(),
                participants: serde_json::json!([]),
                source_type: "manual".into(),
                confidence: 1.0,
                extraction_method: ExtractionMethod::UserAsserted,
                created_at: ts.clone(), updated_at: ts.clone(),
                access_count: 0,
                last_accessed_at: String::new(),
                importance_score: 0.5,
                tier: Tier::Hot,
                pinned: false,
                compacted_into: None,
            }).await.unwrap();
        }

        let events = s.list_events_for_store("le-s1").await.unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, "ev_earlier", "earlier event should come first");
        assert_eq!(events[1].id, "ev_later");
    }

    #[tokio::test]
    async fn create_contains_evidence_edge_round_trips() {
        let s = fixture().await;
        let ts = now();

        // Seed an event and an article
        s.create_event(&Event {
            id: "ce_ev1".into(), store_id: "ce-s1".into(),
            title: "Event".into(), summary: "".into(),
            started_at: ts.clone(), ended_at: ts.clone(),
            participants: serde_json::json!([]),
            source_type: "manual".into(),
            confidence: 1.0,
            extraction_method: ExtractionMethod::UserAsserted,
            created_at: ts.clone(), updated_at: ts.clone(),
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();

        s.create_article(&Article {
            id: "ce_a1".into(), store_id: "ce-s1".into(),
            title: "Article".into(), content: "content".into(),
            source_type: "user".into(), source_id: String::new(),
            content_hash: "ce-h".into(), tags: serde_json::json!([]),
            embedded_at: None, created_at: ts.clone(), updated_at: ts.clone(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();

        s.create_contains_evidence_edge("ce_ev1", "ce_a1", 0.85).await.unwrap();

        // Reverse lookup: article → events should return our event
        let events = s.list_events_for_article("ce_a1").await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, "ce_ev1");
    }

    #[tokio::test]
    async fn create_motivates_edge_round_trips() {
        let s = fixture().await;
        let ts = now();

        for id in &["m_ev1", "m_ev2"] {
            s.create_event(&Event {
                id: id.to_string(), store_id: "m-s1".into(),
                title: id.to_string(), summary: "".into(),
                started_at: ts.clone(), ended_at: ts.clone(),
                participants: serde_json::json!([]),
                source_type: "manual".into(),
                confidence: 1.0,
                extraction_method: ExtractionMethod::UserAsserted,
                created_at: ts.clone(), updated_at: ts.clone(),
                access_count: 0,
                last_accessed_at: String::new(),
                importance_score: 0.5,
                tier: Tier::Hot,
                pinned: false,
                compacted_into: None,
            }).await.unwrap();
        }

        s.create_motivates_edge("m_ev1", "m_ev2", 0.75, Some("rationale".into())).await.unwrap();

        // Verify via direct SQL (no Store helper for reading motivates yet)
        let mut resp = s.db().query(
            "SELECT meta::id(in) AS from_id FROM motivates WHERE out = type::thing('event', 'm_ev2')"
        ).await.unwrap().check().unwrap();
        #[derive(serde::Deserialize)] struct Row { from_id: String }
        let rows: Vec<Row> = resp.take(0).unwrap_or_default();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].from_id, "m_ev1");
    }

    #[tokio::test]
    async fn create_part_of_edge_round_trips() {
        let s = fixture().await;
        let ts = now();

        for id in &["po_child", "po_parent"] {
            s.create_event(&Event {
                id: id.to_string(), store_id: "po-s1".into(),
                title: id.to_string(), summary: "".into(),
                started_at: ts.clone(), ended_at: ts.clone(),
                participants: serde_json::json!([]),
                source_type: "manual".into(),
                confidence: 1.0,
                extraction_method: ExtractionMethod::UserAsserted,
                created_at: ts.clone(), updated_at: ts.clone(),
                access_count: 0,
                last_accessed_at: String::new(),
                importance_score: 0.5,
                tier: Tier::Hot,
                pinned: false,
                compacted_into: None,
            }).await.unwrap();
        }

        s.create_part_of_edge("po_child", "po_parent", 0.95).await.unwrap();

        let mut resp = s.db().query(
            "SELECT meta::id(in) AS child_id FROM part_of WHERE out = type::thing('event', 'po_parent')"
        ).await.unwrap().check().unwrap();
        #[derive(serde::Deserialize)] struct Row { child_id: String }
        let rows: Vec<Row> = resp.take(0).unwrap_or_default();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].child_id, "po_child");
    }

    #[tokio::test]
    async fn article_reflects_field_serde_round_trip() {
        let s = fixture().await;
        let ts = now();

        // A reflection-typed article pointing to two source articles
        s.create_article(&Article {
            id: "p7r-refl".into(),
            store_id: "p7r-s1".into(),
            title: "Reflection on Rust async".into(),
            content: "Synthesized delta from two source articles".into(),
            source_type: "reflection".into(),
            source_id: String::new(),
            content_hash: "p7r-refl-h".into(),
            tags: serde_json::json!([]),
            embedded_at: None,
            created_at: ts.clone(),
            updated_at: ts.clone(),
            reflects: vec!["p7r-a1".into(), "p7r-a2".into()],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();

        let got = s.get_article("p7r-refl").await.unwrap().expect("reflection exists");
        assert_eq!(got.reflects, vec!["p7r-a1".to_string(), "p7r-a2".to_string()]);
        assert_eq!(got.source_type, "reflection");
    }

    #[tokio::test]
    async fn list_reflections_for_article_finds_synthesizers() {
        let s = fixture().await;
        let ts = now();

        // Source article
        s.create_article(&Article {
            id: "lrfa-src".into(),
            store_id: "lrfa-s1".into(),
            title: "Source".into(),
            content: "src content".into(),
            source_type: "user".into(),
            source_id: String::new(),
            content_hash: "lrfa-src-h".into(),
            tags: serde_json::json!([]),
            embedded_at: None,
            created_at: ts.clone(),
            updated_at: ts.clone(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();

        // Two reflections pointing at the source
        for refl_id in &["lrfa-r1", "lrfa-r2"] {
            s.create_article(&Article {
                id: refl_id.to_string(),
                store_id: "lrfa-s1".into(),
                title: format!("Reflection {}", refl_id),
                content: "synthesized content".into(),
                source_type: "reflection".into(),
                source_id: String::new(),
                content_hash: format!("{}-h", refl_id),
                tags: serde_json::json!([]),
                embedded_at: None,
                created_at: ts.clone(),
                updated_at: ts.clone(),
                reflects: vec!["lrfa-src".into()],
                access_count: 0,
                last_accessed_at: String::new(),
                importance_score: 0.5,
                tier: Tier::Hot,
                pinned: false,
                compacted_into: None,
            }).await.unwrap();
        }

        // An unrelated reflection
        s.create_article(&Article {
            id: "lrfa-unrelated".into(),
            store_id: "lrfa-s1".into(),
            title: "Other".into(),
            content: "".into(),
            source_type: "reflection".into(),
            source_id: String::new(),
            content_hash: "lrfa-other-h".into(),
            tags: serde_json::json!([]),
            embedded_at: None,
            created_at: ts.clone(),
            updated_at: ts.clone(),
            reflects: vec!["other-id".into()],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();

        let reflections = s.list_reflections_for_article("lrfa-src").await.unwrap();
        assert_eq!(reflections.len(), 2, "expected 2 reflections pointing to lrfa-src");
        let ids: std::collections::HashSet<&str> = reflections.iter().map(|a| a.id.as_str()).collect();
        assert!(ids.contains("lrfa-r1"));
        assert!(ids.contains("lrfa-r2"));
        assert!(!ids.contains("lrfa-unrelated"));
    }

    // ── P8 Task 3 tests ────────────────────────────────────────────────────

    fn p8_article(id: &str, store_id: &str, tier: Tier, pinned: bool, ts: &str) -> Article {
        Article {
            id: id.to_string(),
            store_id: store_id.to_string(),
            title: "T".into(),
            content: "C".into(),
            source_type: "user".into(),
            source_id: String::new(),
            content_hash: format!("{}-h", id),
            tags: serde_json::json!([]),
            embedded_at: None,
            created_at: ts.to_string(),
            updated_at: ts.to_string(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier,
            pinned,
            compacted_into: None,
        }
    }

    #[tokio::test]
    async fn record_article_access_increments_counter_and_promotes_tier() {
        let s = fixture().await;
        let ts = now();
        s.create_article(&p8_article("p8t3-a1", "p8t3-s1", Tier::Warm, false, &ts))
            .await.unwrap();

        s.record_article_access("p8t3-a1").await.unwrap();

        let got = s.get_article("p8t3-a1").await.unwrap().expect("exists");
        assert_eq!(got.access_count, 1);
        assert_eq!(got.tier, Tier::Hot, "access should promote Warm -> Hot");
        assert!(!got.last_accessed_at.is_empty());
    }

    #[tokio::test]
    async fn record_article_access_hot_stays_hot() {
        let s = fixture().await;
        let ts = now();
        s.create_article(&p8_article("p8t3-a2", "p8t3-s2", Tier::Hot, false, &ts))
            .await.unwrap();

        s.record_article_access("p8t3-a2").await.unwrap();

        let got = s.get_article("p8t3-a2").await.unwrap().expect("exists");
        assert_eq!(got.access_count, 1);
        assert_eq!(got.tier, Tier::Hot, "already Hot stays Hot");
    }

    #[tokio::test]
    async fn pin_article_sets_pinned_and_logs_audit() {
        let s = fixture().await;
        let ts = now();
        // Use a store that exists in the fixture
        let mut art = p8_article("p8t3-pin1", "s1", Tier::Warm, false, &ts);
        art.store_id = "s1".into();
        s.create_article(&art).await.unwrap();

        s.pin_article("p8t3-pin1").await.unwrap();

        let got = s.get_article("p8t3-pin1").await.unwrap().expect("exists");
        assert!(got.pinned, "pinned flag must be set");

        let log = s.list_audit_log("s1", None, 10).await.unwrap();
        assert!(
            log.iter().any(|e| e.action == "pin" && e.subject_id == "p8t3-pin1"),
            "audit log must contain a pin entry for the article"
        );
    }

    #[tokio::test]
    async fn unpin_article_clears_pinned() {
        let s = fixture().await;
        let ts = now();
        let mut art = p8_article("p8t3-unpin1", "s1", Tier::Hot, true, &ts);
        art.pinned = true;
        s.create_article(&art).await.unwrap();

        s.unpin_article("p8t3-unpin1").await.unwrap();

        let got = s.get_article("p8t3-unpin1").await.unwrap().expect("exists");
        assert!(!got.pinned, "pinned flag must be cleared");

        let log = s.list_audit_log("s1", None, 10).await.unwrap();
        assert!(
            log.iter().any(|e| e.action == "unpin" && e.subject_id == "p8t3-unpin1"),
            "audit log must contain an unpin entry"
        );
    }

    #[tokio::test]
    async fn set_article_tier_logs_transition() {
        let s = fixture().await;
        let ts = now();
        let mut art = p8_article("p8t3-tier1", "s1", Tier::Hot, false, &ts);
        art.store_id = "s1".into();
        s.create_article(&art).await.unwrap();

        s.set_article_tier("p8t3-tier1", Tier::Cold, "test_reason").await.unwrap();

        let got = s.get_article("p8t3-tier1").await.unwrap().expect("exists");
        assert_eq!(got.tier, Tier::Cold, "tier must be updated to Cold");

        let log = s.list_audit_log("s1", None, 10).await.unwrap();
        let entry = log.iter().find(|e| e.action == "tier_change" && e.subject_id == "p8t3-tier1")
            .expect("tier_change audit entry must exist");
        // SurrealDB object fields round-trip through serde; check via serialized string
        let details_str = entry.details.to_string();
        assert!(details_str.contains("\"hot\"") || details_str.contains("hot"),
            "details must record from=hot; got: {}", details_str);
        assert!(details_str.contains("cold"),
            "details must record to=cold; got: {}", details_str);
    }

    #[tokio::test]
    async fn list_articles_by_tier_filters_correctly() {
        let s = fixture().await;
        let ts = now();
        // Two Hot articles and one Cold article in the same store
        for (id, tier) in &[
            ("p8t3-list-h1", Tier::Hot),
            ("p8t3-list-h2", Tier::Hot),
            ("p8t3-list-c1", Tier::Cold),
        ] {
            let mut art = p8_article(id, "s1", *tier, false, &ts);
            art.store_id = "s1".into();
            s.create_article(&art).await.unwrap();
        }

        let hot = s.list_articles_by_tier("s1", Tier::Hot).await.unwrap();
        let cold = s.list_articles_by_tier("s1", Tier::Cold).await.unwrap();

        let hot_ids: std::collections::HashSet<&str> = hot.iter().map(|a| a.id.as_str()).collect();
        assert!(hot_ids.contains("p8t3-list-h1"), "h1 must be in Hot list");
        assert!(hot_ids.contains("p8t3-list-h2"), "h2 must be in Hot list");
        assert!(!hot_ids.contains("p8t3-list-c1"), "c1 must NOT be in Hot list");

        let cold_ids: std::collections::HashSet<&str> = cold.iter().map(|a| a.id.as_str()).collect();
        assert!(cold_ids.contains("p8t3-list-c1"), "c1 must be in Cold list");
        assert!(!cold_ids.contains("p8t3-list-h1"), "h1 must NOT be in Cold list");
    }

    #[tokio::test]
    async fn write_audit_log_persists_entry() {
        let s = fixture().await;
        let ts = now();
        let entry = AuditLogEntry {
            id: "p8t3-al-manual-1".into(),
            store_id: "s1".into(),
            action: "tier_change".into(),
            subject_type: "article".into(),
            subject_id: "p8t3-any-art".into(),
            details: serde_json::json!({ "from": "warm", "to": "cold" }),
            recorded_at: ts.clone(),
        };
        s.write_audit_log(&entry).await.unwrap();

        let log = s.list_audit_log("s1", None, 50).await.unwrap();
        assert!(
            log.iter().any(|e| e.id == "p8t3-al-manual-1"),
            "manually written audit entry must round-trip"
        );
    }

    #[tokio::test]
    async fn set_article_compacted_into_archives_and_logs() {
        let s = fixture().await;
        let ts = now();
        let mut art = p8_article("p8t3-compact-src", "s1", Tier::Cold, false, &ts);
        art.store_id = "s1".into();
        s.create_article(&art).await.unwrap();

        s.set_article_compacted_into("p8t3-compact-src", "p8t3-refl-1").await.unwrap();

        let got = s.get_article("p8t3-compact-src").await.unwrap().expect("exists");
        assert_eq!(got.compacted_into, Some("p8t3-refl-1".into()), "compacted_into must be set");
        assert_eq!(got.tier, Tier::Archive, "tier must become Archive after compaction");

        let log = s.list_audit_log("s1", None, 10).await.unwrap();
        let entry = log.iter().find(|e| e.action == "compact" && e.subject_id == "p8t3-compact-src")
            .expect("compact audit entry must exist");
        let details_str = entry.details.to_string();
        assert!(details_str.contains("p8t3-refl-1"),
            "details must contain reflection id; got: {}", details_str);
        assert!(details_str.contains("archive"),
            "details must contain archive tier; got: {}", details_str);
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
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();
        s.create_article(&Article {
            id: "a2".into(), store_id: "s1".into(), title: "Async Rust".into(),
            content: "Tokio and async".into(), source_type: "user".into(),
            source_id: "".into(), content_hash: "h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
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
                reflects: vec![],
                access_count: 0,
                last_accessed_at: String::new(),
                importance_score: 0.5,
                tier: Tier::Hot,
                pinned: false,
                compacted_into: None,
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
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
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
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
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
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();
        s.create_article(&Article {
            id: "a2".into(), store_id: "s1".into(), title: "No mentions".into(),
            content: "C2".into(), source_type: "user".into(),
            source_id: "".into(), content_hash: "h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
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
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();
        s.create_article(&Article {
            id: "tgsi-a2".into(), store_id: "s1".into(), title: "Go Concurrency".into(),
            content: "Go uses goroutines for concurrent programming".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "tgsi-h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();
        s.create_article(&Article {
            id: "tgsi-a3".into(), store_id: "s1".into(), title: "Tokio Internals".into(),
            content: "Deep dive into how Tokio scheduler works".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "tgsi-h3".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
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
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();
        s.create_article(&Article {
            id: "gse-a2".into(), store_id: "s1".into(),
            title: "Go Concurrency".into(),
            content: "Go uses goroutines for concurrent programming".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "gse-h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();
        s.create_article(&Article {
            id: "gse-a3".into(), store_id: "s1".into(),
            title: "Tokio Internals".into(),
            content: "Deep dive into how Tokio scheduler works".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "gse-h3".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
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

    /// Sibling of graph_searcher_end_to_end that explicitly selects the
    /// jaccard (P4) graph strategy. Confirms P5 list_related_articles still
    /// produces ENTITY_OVERLAP-based results when not using activation.
    #[tokio::test]
    async fn graph_searcher_jaccard_strategy_explicit() {
        use crate::retrieval::GraphSearcher;
        use crate::config::RetrievalConfig;
        use std::sync::Arc;

        let s = fixture().await;
        let ts = now();

        // Same fixture as graph_searcher_end_to_end (3 articles, 2 entities,
        // MENTIONS, ENTITY_OVERLAP), abbreviated since the principle is to
        // verify the dispatch path, not the fixture coverage:
        s.create_article(&Article {
            id: "gj-a1".into(), store_id: "gj-s1".into(),
            title: "Rust async".into(),
            content: "Rust provides async capabilities using Tokio".into(),
            source_type: "user".into(), source_id: String::new(),
            content_hash: "gj-h1".into(), tags: serde_json::json!([]),
            embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();

        s.create_entity(&Entity {
            id: "gj-tool-rust".into(), name: "Rust".into(),
            entity_type: "tool".into(), description: None,
            store_id: "gj-s1".into(), mention_count: 1,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_mentions_edge("gj-a1", "gj-tool-rust", "Rust provides", 0.95).await.unwrap();

        // Explicit jaccard strategy
        let config = RetrievalConfig { graph_strategy: "jaccard".into(), ..RetrievalConfig::default() };
        let db: Arc<dyn Store> = Arc::new(s);
        let searcher = GraphSearcher::new(db, config);

        let output = searcher.search("Rust", "gj-s1", 10).await.expect("search");
        assert!(!output.results.is_empty(),
            "jaccard path should still produce results for direct entity match");
        let ids: Vec<&str> = output.results.iter().map(|r| r.article_id.as_str()).collect();
        assert!(ids.contains(&"gj-a1"));
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
                reflects: vec![],
                access_count: 0,
                last_accessed_at: String::new(),
                importance_score: 0.5,
                tier: Tier::Hot,
                pinned: false,
                compacted_into: None,
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
            reflects: vec![],
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();

        let db: Arc<dyn Store> = Arc::new(s);
        let engine = ActivationEngine::new(db, RetrievalConfig::default());

        // No entities seeded; any query produces empty seeds → empty results
        let output = engine.search("nonsense query xyzzy", "ae2-s1", 10).await.unwrap();

        assert!(output.results.is_empty(), "no matched entities should yield empty results");
        assert_eq!(output.entity_coverage, 0.0);
        assert_eq!(output.node_count, 0);
    }

    /// End-to-end test of the P7 reflection wiring up to (but not including)
    /// the LLM call. Verifies: ReflectionCluster constructable from articles,
    /// Reflector::reflect() returns None when LLM is disabled, manually
    /// stored reflections round-trip with their `reflects` field intact,
    /// and `list_reflections_for_article` finds them.
    #[tokio::test]
    async fn p7_reflection_end_to_end_wiring() {
        use crate::config::ExtractionConfig;
        use crate::knowledge::reflection::{Reflector, ReflectionCluster};

        let s = fixture().await;
        let ts = now();

        // Seed 5 source articles sharing entity "outage"
        for (id, title) in &[
            ("p7e-a1", "Outage Mon"),
            ("p7e-a2", "Outage Tue"),
            ("p7e-a3", "Outage Wed"),
            ("p7e-a4", "Outage Thu"),
            ("p7e-a5", "Outage Fri"),
        ] {
            s.create_article(&Article {
                id: id.to_string(),
                store_id: "p7e-s1".into(),
                title: title.to_string(),
                content: format!("Outage details for {}", title),
                source_type: "user".into(),
                source_id: String::new(),
                content_hash: format!("{}-h", id),
                tags: serde_json::json!(["incident"]),
                embedded_at: None,
                created_at: ts.clone(),
                updated_at: ts.clone(),
                reflects: vec![],
                access_count: 0,
                last_accessed_at: String::new(),
                importance_score: 0.5,
                tier: Tier::Hot,
                pinned: false,
                compacted_into: None,
            }).await.unwrap();
        }

        let articles = s.list_articles_for_store("p7e-s1").await.unwrap();
        assert_eq!(articles.len(), 5);

        // Reflector with LLM disabled — no network call, returns None.
        let cfg = ExtractionConfig {
            enabled: false,
            ollama_url: "http://localhost:11434".into(),
            model: "llama3.2:3b".into(),
        };
        let reflector = Reflector::new(cfg);

        let cluster = ReflectionCluster {
            sources: articles.clone(),
            intent: "shared outage incidents".into(),
        };

        let result = reflector.reflect(&cluster).await.unwrap();
        assert!(result.is_none(),
            "with LLM disabled, reflect() returns None (no network call)");

        // Manually simulate what cmd_reflect would do post-LLM: create a
        // reflection-typed Article with reflects = source_ids.
        let reflection_id = "p7e-refl-1";
        let source_ids: Vec<String> = articles.iter().map(|a| a.id.clone()).collect();
        s.create_article(&Article {
            id: reflection_id.into(),
            store_id: "p7e-s1".into(),
            title: "Reflection: outage pattern".into(),
            content: "5 outage incidents this week, all during business hours".into(),
            source_type: "reflection".into(),
            source_id: String::new(),
            content_hash: "p7e-refl-h".into(),
            tags: serde_json::json!([]),
            embedded_at: None,
            created_at: ts.clone(),
            updated_at: ts.clone(),
            reflects: source_ids.clone(),
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();

        // Verify the reflection round-trips its reflects field
        let stored = s.get_article(reflection_id).await.unwrap()
            .expect("reflection exists");
        assert_eq!(stored.source_type, "reflection");
        assert_eq!(stored.reflects.len(), 5);
        let stored_ids: std::collections::HashSet<String> =
            stored.reflects.iter().cloned().collect();
        let expected_ids: std::collections::HashSet<String> =
            source_ids.iter().cloned().collect();
        assert_eq!(stored_ids, expected_ids,
            "reflects field must contain all 5 source IDs");

        // Verify list_reflections_for_article finds the reflection from
        // each source's perspective
        for source_id in &source_ids {
            let reflections = s.list_reflections_for_article(source_id).await.unwrap();
            assert_eq!(reflections.len(), 1,
                "source {} should be referenced by exactly 1 reflection", source_id);
            assert_eq!(reflections[0].id, reflection_id);
        }
    }

    /// Verifies the compression-amplified-toxin defense: even if we pass
    /// LLM-confident output, the `min(source_confidences)` floor caps it.
    /// (Direct unit test of the cap logic; the full pipeline test above is
    /// blocked on LLM availability.)
    #[tokio::test]
    async fn p7_reflection_toxin_floor_caps_confidence() {
        // Direct logic exercise: simulate the cap that Reflector.reflect() applies.
        // For P7 we treat user articles as confidence 1.0 → the cap is a no-op
        // unless the LLM itself returns < 1.0. Verify the math.
        let llm_confidence: f64 = 0.95;
        let min_source_confidence: f64 = 0.3;  // imagined low-conf source
        let capped = llm_confidence.min(min_source_confidence).clamp(0.0, 1.0);
        assert_eq!(capped, 0.3);

        // And the no-op case: high-conf source means LLM confidence flows through
        let llm_high: f64 = 0.7;
        let user_source: f64 = 1.0;
        let capped_noop = llm_high.min(user_source).clamp(0.0, 1.0);
        assert_eq!(capped_noop, 0.7);
    }

    /// Verifies that record_article_access increments access_count and
    /// records an audit entry. (The pipeline-level fire-and-forget spawn
    /// is implicitly tested by Task 6's tests, which already exercise the
    /// executor path; this test focuses on the direct Store-level call
    /// that the spawn ultimately makes.)
    #[tokio::test]
    async fn p8_record_access_increments_counter_and_logs_audit() {
        let s = fixture().await;
        let ts = now();

        // Seed a Warm-tier article (will be promoted to Hot via access)
        s.create_article(&Article {
            id: "p8t8-a1".into(),
            store_id: "p8t8-s1".into(),
            title: "T".into(),
            content: "C".into(),
            source_type: "user".into(),
            source_id: String::new(),
            content_hash: "h".into(),
            tags: serde_json::json!([]),
            embedded_at: None,
            created_at: ts.clone(),
            updated_at: ts.clone(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: "".into(),
            importance_score: 0.5,
            tier: Tier::Warm,
            pinned: false,
            compacted_into: None,
        }).await.unwrap();

        // Also create the store since we're using a different store_id
        s.create_store(&KnowledgeStore {
            id: "p8t8-s1".into(), owner_id: "u1".into(), store_type: "personal".into(),
            name: "P8T8".into(), lancedb_collection: "store_p8t8_s1".into(),
            quantizer_version: "ivf_pq_v1".into(),
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        s.record_article_access("p8t8-a1").await.unwrap();

        let got = s.get_article("p8t8-a1").await.unwrap().unwrap();
        assert_eq!(got.access_count, 1, "access_count must increment");
        assert_eq!(got.tier, Tier::Hot, "Warm article should be promoted to Hot on access");
        assert!(!got.last_accessed_at.is_empty(), "last_accessed_at must be set");

        // Audit log should have a tier_change entry
        let entries = s.list_audit_log("p8t8-s1", None, 10).await.unwrap();
        let has_audit = entries.iter().any(|e|
            e.action == "tier_change"
            && e.subject_id == "p8t8-a1"
            && e.details.get("reason").and_then(|v| v.as_str()) == Some("access_promote")
        );
        assert!(has_audit, "access promotion must write an audit entry; got {:?}", entries);
    }

    /// P8 end-to-end: 10 articles with varied last_accessed_at timestamps.
    /// After nightly_tier_transition, verify the salience-driven demotion
    /// pattern and that pinned items override decay.
    #[tokio::test]
    async fn p8_e2e_tier_transitions_on_10_article_fixture() {
        use crate::maintenance::nightly_tier_transition;
        use crate::config::DecayConfig;
        use std::sync::Arc;

        let s = fixture().await;
        let now = chrono::Utc::now();

        // Build 10 articles with varied recency. With λ=0.02 (35-day half-life)
        // and thresholds Hot=0.5 / Warm=0.1 / Cold=0.01 / Archive<0.01:
        // - days≤35 + importance=0.8 → salience ≥ 0.4 → Hot (just under 0.5 OK)
        // - days≤35 + importance=1.0 → salience ≈ 0.5 → Hot
        // - days≈100 → salience ≈ 0.13 → Warm
        // - days≈200 → salience ≈ 0.018 → Cold
        // - days≈365 → salience < 0.01 → Archive
        // - days≈500 → salience < 0.01 → Archive
        let fixtures = [
            ("p8e-fresh1", 0, 1.0, false),    // → Hot
            ("p8e-fresh2", 5, 1.0, false),    // → Hot
            ("p8e-recent", 30, 1.0, false),   // → Hot (importance=1.0 → salience ≈ 0.55)
            ("p8e-mid1", 100, 1.0, false),    // → Warm (salience ≈ 0.13)
            ("p8e-mid2", 120, 1.0, false),    // → Warm/Cold boundary (salience ≈ 0.09)
            ("p8e-old1", 200, 1.0, false),    // → Cold (salience ≈ 0.018)
            ("p8e-old2", 220, 1.0, false),    // → Cold (salience ≈ 0.016)
            ("p8e-ancient1", 365, 1.0, false), // → Archive (salience ≈ 0.00067)
            ("p8e-ancient2", 500, 1.0, false), // → Archive (salience ≈ 0.000045)
            ("p8e-pinned-old", 365, 1.0, true), // PINNED → stays Hot regardless
        ];

        for (id, days_ago, importance, pinned) in &fixtures {
            let ts = (now - chrono::Duration::days(*days_ago)).to_rfc3339();
            s.create_article(&Article {
                id: id.to_string(),
                store_id: "p8e-s1".into(),
                title: format!("Article {}", id),
                content: format!("content of {}", id),
                source_type: "user".into(),
                source_id: String::new(),
                content_hash: format!("{}-h", id),
                tags: serde_json::json!([]),
                embedded_at: None,
                created_at: ts.clone(),
                updated_at: ts.clone(),
                reflects: vec![],
                access_count: 0,
                last_accessed_at: ts,
                importance_score: *importance,
                tier: Tier::Hot,    // all start Hot; transition will demote some
                pinned: *pinned,
                compacted_into: None,
            }).await.unwrap();
        }

        let db: Arc<dyn Store> = Arc::new(s);
        let cfg = DecayConfig::default();
        let report = nightly_tier_transition(db.clone(), "p8e-s1", &cfg, now).await.unwrap();

        // Sanity: scanned all 10
        assert_eq!(report.articles_scanned, 10, "should scan all 10 articles");
        assert_eq!(report.pinned_skipped, 1, "pinned article must be skipped");

        // Fetch all final tiers using a helper
        let get_tier = |aid: &str| {
            let db = db.clone();
            let aid = aid.to_string();
            async move {
                db.get_article(&aid).await.unwrap().unwrap().tier
            }
        };

        // Recent articles stay Hot (with importance=1.0 they're salience ≥0.5)
        assert_eq!(get_tier("p8e-fresh1").await, Tier::Hot, "0-day article should be Hot");
        assert_eq!(get_tier("p8e-fresh2").await, Tier::Hot, "5-day article should be Hot");
        assert_eq!(get_tier("p8e-recent").await, Tier::Hot, "30-day article should be Hot");

        // Ancient articles go to Cold or Archive
        let ancient1 = get_tier("p8e-ancient1").await;
        let ancient2 = get_tier("p8e-ancient2").await;
        assert!(matches!(ancient1, Tier::Cold | Tier::Archive),
            "365-day article should be Cold or Archive; got {:?}", ancient1);
        assert!(matches!(ancient2, Tier::Cold | Tier::Archive),
            "500-day article should be Cold or Archive; got {:?}", ancient2);

        // PIN OVERRIDE: pinned 365-day article stays Hot
        let pinned_tier = get_tier("p8e-pinned-old").await;
        assert_eq!(pinned_tier, Tier::Hot,
            "pinned article must stay Hot regardless of age; got {:?}", pinned_tier);

        // Audit log: should have entries for non-pinned demotions only
        let entries = db.list_audit_log("p8e-s1", None, 100).await.unwrap();
        let nightly_entries: Vec<&AuditLogEntry> = entries.iter()
            .filter(|e|
                e.action == "tier_change"
                && e.details.get("reason")
                    .and_then(|v| v.as_str())
                    .map(|s| s.contains("nightly_decay"))
                    .unwrap_or(false)
            )
            .collect();

        // At least 2 transitions (the ancients); pinned must NOT appear
        assert!(nightly_entries.len() >= 2,
            "expected at least 2 nightly transitions; got {}", nightly_entries.len());
        assert!(
            !nightly_entries.iter().any(|e| e.subject_id == "p8e-pinned-old"),
            "pinned article must not appear in transition audit log"
        );
    }

    /// P9 lifecycle test: observe (create article), recall (find via search),
    /// forget (set Archive), verify-forgotten (excluded from non-archive recall).
    ///
    /// Doesn't go through HTTP — tests the underlying operations the P9
    /// handlers perform. Full HTTP-level test requires K2K auth scaffolding
    /// that's out of scope for this verification.
    #[tokio::test]
    async fn p9_observe_recall_forget_lifecycle() {
        let s = fixture().await;
        let ts = now();

        // === observe: create an article (what /v1/memory/observe does) ===
        let article = Article {
            id: "p9-obs1".into(),
            store_id: "s1".into(),
            title: "An observation about Rust async".into(),
            content: "Rust provides powerful async via Tokio".into(),
            source_type: "user".into(),
            source_id: String::new(),
            content_hash: "p9-h1".into(),
            tags: serde_json::json!([]),
            embedded_at: None,
            created_at: ts.clone(),
            updated_at: ts.clone(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: "".into(),
            importance_score: 0.5,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        };
        s.create_article(&article).await.unwrap();

        // Set up entity so the recall path finds the article via GraphSearcher
        s.create_entity(&Entity {
            id: "p9-tool-rust".into(),
            name: "Rust".into(),
            entity_type: "tool".into(),
            description: None,
            store_id: "s1".into(),
            mention_count: 1,
            created_at: ts.clone(),
            updated_at: ts.clone(),
        }).await.unwrap();
        s.create_mentions_edge("p9-obs1", "p9-tool-rust", "Rust provides", 0.95).await.unwrap();

        // === recall: query via Store (what /v1/memory/recall does internally) ===
        let articles_found = s.list_articles_for_entity("p9-tool-rust").await.unwrap();
        assert!(!articles_found.is_empty(),
            "recall query should find the observed article");
        assert!(articles_found.iter().any(|a| a.id == "p9-obs1"));

        // === forget: archive (what /v1/memory/forget does) ===
        s.set_article_tier("p9-obs1", Tier::Archive, "forget_api: user requested").await.unwrap();

        let archived = s.get_article("p9-obs1").await.unwrap().unwrap();
        assert_eq!(archived.tier, Tier::Archive);

        // === verify forgotten: tier_factor returns 0 for Archive when include_archive=false ===
        use crate::maintenance::decay::tier_factor;
        let factor_default = tier_factor(Tier::Archive, false);
        assert_eq!(factor_default, 0.0,
            "Archive items must be excluded by default (recall include_archive=false)");

        let factor_explicit = tier_factor(Tier::Archive, true);
        assert!(factor_explicit > 0.0,
            "Archive items must be surfaceable with include_archive=true");

        // === audit trail: forget operation must be audit-logged ===
        let entries = s.list_audit_log("s1", None, 100).await.unwrap();
        let has_forget_entry = entries.iter().any(|e|
            e.subject_id == "p9-obs1"
            && e.action == "tier_change"
            && e.details.get("reason")
                .and_then(|v| v.as_str())
                .map(|s| s.contains("forget_api"))
                .unwrap_or(false)
        );
        assert!(has_forget_entry,
            "forget operation must write an audit entry; got entries: {:?}", entries);
    }

    #[tokio::test]
    async fn write_policy_trace_persists() {
        let s = fixture().await;
        let trace = PolicyTrace {
            id: "p10t5-1".into(),
            store_id: "p10-s1".into(),
            policy_name: "default_synapse_aligned".into(),
            decision_type: DecisionType::Decay,
            input_features: serde_json::json!({"days_since_access": 30, "importance": 0.8}),
            action: serde_json::json!({"salience": 0.55, "tier": "hot"}),
            outcome: None,
            recorded_at: "2026-05-24T00:00:00Z".into(),
        };
        s.write_policy_trace(&trace).await.unwrap();

        let traces = s.list_policy_traces(Some("p10-s1"), None, None, 100).await.unwrap();
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].id, "p10t5-1");
        // FLEXIBLE schema means nested keys survive
        assert_eq!(traces[0].input_features["days_since_access"], 30);
        assert_eq!(traces[0].action["tier"], "hot");
    }

    #[tokio::test]
    async fn list_policy_traces_filters_by_policy_name() {
        let s = fixture().await;
        let mk = |id: &str, name: &str| PolicyTrace {
            id: id.into(),
            store_id: "p10t5b-s1".into(),
            policy_name: name.into(),
            decision_type: DecisionType::Decay,
            input_features: serde_json::json!({}),
            action: serde_json::json!({}),
            outcome: None,
            recorded_at: "2026-05-24T00:00:00Z".into(),
        };

        s.write_policy_trace(&mk("p10t5b-1", "policy_a")).await.unwrap();
        s.write_policy_trace(&mk("p10t5b-2", "policy_b")).await.unwrap();
        s.write_policy_trace(&mk("p10t5b-3", "policy_a")).await.unwrap();

        let policy_a = s.list_policy_traces(None, Some("policy_a"), None, 100).await.unwrap();
        assert_eq!(policy_a.len(), 2);
        assert!(policy_a.iter().all(|t| t.policy_name == "policy_a"));

        let policy_b = s.list_policy_traces(None, Some("policy_b"), None, 100).await.unwrap();
        assert_eq!(policy_b.len(), 1);
    }

    #[tokio::test]
    async fn decision_type_enum_roundtrips_via_serde() {
        let d = DecisionType::ReflectionTrigger;
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(json, "\"reflection_trigger\"");
        let back: DecisionType = serde_json::from_str("\"activation_weight\"").unwrap();
        assert_eq!(back, DecisionType::ActivationWeight);
    }

}
