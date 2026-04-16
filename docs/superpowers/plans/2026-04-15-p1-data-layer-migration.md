# P1 — Data Layer Migration (SQLite → SurrealDB) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the SQLite + FTS5 data layer with an embedded SurrealDB (`kv-surrealkv` backend) that preserves every existing relational and full-text search behavior of knowledge-nexus-local 0.8, ships a one-shot `migrate --from sqlite --to surrealdb` command, and leaves the codebase wired to a new `Store` trait.

**Architecture:** New `src/store/` module with a `Store` trait and a single `SurrealStore` impl. SurrealDB schema is SCHEMAFULL. Article gets two new fields (`content_hash`, `source_id`) and `knowledge_store` gets `quantizer_version` now, so later phases don't need another migration. Tags stay as a JSON field on `article` through P1 (P3 will normalize them into a `tag` table). Graph edges, entity extraction, and dedup are explicitly **out of scope** for P1. Legacy `src/db/` stays alive only long enough for the migrator to read the old SQLite file, then gets deleted.

**Tech Stack:**
- `surrealdb = { version = "2", default-features = false, features = ["kv-surrealkv", "kv-mem"] }`
- `rusqlite` — kept for the migration window only; deleted at the end of P1.
- `sha2` — already a dep; used for `content_hash` backfill.
- Existing `tokio`, `anyhow`, `serde`, `chrono` stay.

**P1 Ship Criteria (from spec §13, scoped to this phase):**
1. `migrate --from sqlite --to surrealdb` works end-to-end on a representative 0.8 DB, verified by the integration test at `tests/migration_sqlite_to_surreal.rs`.
2. All pre-existing tests and CLI commands (`Init`, `Start`, `Stop`, `Status`, `Config`, `Paths`, `Search`, `Reindex`, `Ui`, `Logs`) work against SurrealDB.
3. Startup time within 20% of 0.8 baseline on a freshly migrated DB.
4. No references to `crate::db::*` remain in `src/` outside `src/migrate/`.

**Out of scope for P1 (belong to P2–P5):**
- `VectorQuantizer` trait and non-IVF_PQ impls (P2)
- Entity extraction, entity/tag tables, `MENTIONS`/`TAGGED`/`RELATED_TO` edges, article + entity dedup (P3)
- Graph-aware hybrid retrieval (P4)
- TurboQuant integration (P5)

**Branch discipline (from `/Users/jared.cluff/gitrepos/CLAUDE.md`):**
All work lands on a feature branch `feat/p1-data-layer-migration`, each task ends with a commit, and a single PR is opened at the end. Commit messages must contain no Claude/AI attribution.

---

## File Structure

**Created in P1:**
```
src/store/
├── mod.rs              Store trait + SurrealStore impl (open, CRUD, FTS)
├── models.rs           Rust types for relational tables (User, Article, ...)
├── schema.rs           SurrealQL DDL (CREATE TABLE/FIELD/INDEX statements)
└── migrations.rs       Schema version tracking + DDL runner

src/migrate/
└── mod.rs              SQLite reader → SurrealDB writer; idempotent upsert;
                        content_hash backfill; migration_completed marker

tests/
└── migration_sqlite_to_surreal.rs
                        End-to-end migration integration test
```

**Modified in P1:**
```
Cargo.toml              Add surrealdb, sha2 features audit, no removals yet
src/main.rs             Add Migrate subcommand; rewire startup to SurrealStore;
                        add SurrealDB-exists-but-no-marker guard
src/k2k/tasks.rs        Replace Arc<Database> with Arc<dyn Store>; .await
src/k2k/handlers.rs     Replace crate::db:: paths with crate::store::
src/k2k/server.rs       Replace Arc<Database> with Arc<dyn Store>; .await
src/k2k/keys.rs         Replace Arc<Database> with Arc<dyn Store>; .await
src/router/mod.rs       Arc<Database> → Arc<dyn Store>
src/router/planner.rs   Arc<Database> → Arc<dyn Store>; .await
src/federation/agreements.rs
                        Arc<Database> → Arc<dyn Store>; .await
src/federation/remote_query.rs
                        Import DiscoveredNode from crate::store::
src/knowledge/articles.rs
                        Arc<Database> → Arc<dyn Store>; .await
src/knowledge/extraction.rs
                        Arc<Database> → Arc<dyn Store>; .await
src/knowledge/conversations.rs
                        Arc<Database> → Arc<dyn Store>; .await
src/retrieval/hybrid.rs Arc<Database> → Arc<dyn Store>; .await on fts/articles
src/discovery/registry.rs
                        Arc<Database> → Arc<dyn Store>; .await
src/connectors/web_clip.rs
                        Arc<Database> → Arc<dyn Store>; .await
```

**Deleted in P1 (at the end, after rewiring):**
```
src/db/
├── mod.rs
├── models.rs
├── migrations.rs
└── repository.rs
```

`rusqlite` stays in `Cargo.toml` through P1 for the migrator to read the old DB. It is removed in a follow-up maintenance PR after 1.0.0 ships (per spec §9 and §12), not in P1.

---

## Task 1: Create the feature branch

**Files:** none

- [ ] **Step 1: Branch off main**

Run:
```bash
cd /Users/jared.cluff/gitrepos/knowledge-nexus-local
git checkout main
git pull --ff-only
git checkout -b feat/p1-data-layer-migration
git status
```
Expected: `On branch feat/p1-data-layer-migration` and clean working tree.

---

## Task 2: Add SurrealDB dependency and confirm it builds

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add the dep under the existing block**

In `Cargo.toml`, directly after the `# Shared K2K library` block (around line 110–111, after the `k2k = { path = "../k2k" }` line), insert:

```toml
# Embedded multi-model DB (relational + FTS + graph) — pure-Rust KV backend.
# kv-mem is used by in-memory unit + integration tests only.
surrealdb = { version = "2", default-features = false, features = ["kv-surrealkv", "kv-mem"] }
```

- [ ] **Step 2: Compile to verify the dep resolves**

Run: `cargo build --no-default-features`
Expected: `Finished ... profile [unoptimized + debuginfo] target(s) in ...`. No compile errors.

(First run downloads `surrealdb` + transitive deps. Subsequent runs are instant.)

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add surrealdb dep for data layer migration"
```

---

## Task 3: Create the empty `store` module with the `Store` trait skeleton

**Files:**
- Create: `src/store/mod.rs`
- Modify: `src/main.rs` (add `mod store;` under `mod db;`)

- [ ] **Step 1: Write the failing trait-exists test**

Create `src/store/mod.rs`:
```rust
//! Repository layer backed by SurrealDB.
//!
//! The `Store` trait abstracts over the concrete backend so tests and future
//! phases (e.g. P3 graph writes, a hypothetical mock backend for the router)
//! can swap in fakes. In 1.0.0 there is exactly one impl: `SurrealStore`.

pub mod migrations;
pub mod models;
pub mod schema;

pub use models::*;

use anyhow::Result;
use async_trait::async_trait;

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
```

Add to `src/main.rs` after `mod db;` (line 16):
```rust
mod store;
mod migrate;
```

(`migrate` is created in Task 17; we declare the module now so one `cargo check` sweep later covers both. Create an empty placeholder `src/migrate/mod.rs` with `//! Placeholder — populated in Task 17.` so the declaration compiles.)

- [ ] **Step 2: Run compile — trait must exist, Store impl not yet**

Run: `cargo build`
Expected: PASS. Nothing yet uses `Store`, so no unresolved references. The trait compiles as dead code.

- [ ] **Step 3: Commit**

```bash
git add src/store/mod.rs src/migrate/mod.rs src/main.rs
git commit -m "store: scaffold Store trait and module structure"
```

---

## Task 4: Port data models to `src/store/models.rs`

**Files:**
- Create: `src/store/models.rs`

The new models are a superset of `src/db/models.rs` with the 1.0.0 fields added: `Article.content_hash`, `Article.source_id`, `KnowledgeStore.quantizer_version`.

- [ ] **Step 1: Write the models file**

Create `src/store/models.rs`:
```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub is_owner: bool,
    pub settings: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeStore {
    pub id: String,
    pub owner_id: String,
    pub store_type: String, // "personal", "family", "shared"
    pub name: String,
    pub lancedb_collection: String,
    /// Which `VectorQuantizer` impl this store uses. Populated by P2 dispatch;
    /// migrated rows default to "ivf_pq_v1" via the schema default.
    pub quantizer_version: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Article {
    pub id: String,
    pub store_id: String,
    pub title: String,
    pub content: String,
    pub source_type: String,
    /// Stable identifier from the source connector (URL, RSS GUID, etc.).
    /// Empty string for articles that predate 1.0.0; connectors populate it
    /// going forward (wired in P3).
    pub source_id: String,
    /// SHA-256 of normalized content. Used by P3 dedup; backfilled during
    /// migration.
    pub content_hash: String,
    /// JSON array of tag strings — normalized into a `tag` table + `TAGGED`
    /// edges in P3. Kept as JSON on the article through P1.
    pub tags: serde_json::Value,
    pub embedded_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub user_id: String,
    pub title: String,
    pub message_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub role: String,
    pub content: String,
    pub metadata: serde_json::Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct K2KClient {
    pub client_id: String,
    pub public_key_pem: String,
    pub client_name: String,
    pub registered_at: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationAgreement {
    pub id: String,
    pub local_store_id: String,
    pub remote_node_id: String,
    pub remote_endpoint: String,
    pub access_type: String, // "read", "write", "readwrite"
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredNode {
    pub node_id: String,
    pub host: String,
    pub port: u16,
    pub endpoint: String,
    pub capabilities: serde_json::Value,
    pub last_seen: String,
    pub healthy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct ConnectorConfig {
    pub id: String,
    pub connector_type: String,
    pub name: String,
    pub config: serde_json::Value,
    pub store_id: String,
    pub created_at: String,
    pub updated_at: String,
}
```

- [ ] **Step 2: Write a round-trip test**

Add at the bottom of `src/store/models.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_article_serde_round_trip_with_new_fields() {
        let a = Article {
            id: "a1".into(),
            store_id: "s1".into(),
            title: "T".into(),
            content: "C".into(),
            source_type: "user".into(),
            source_id: "https://example.com/x".into(),
            content_hash: "abc123".into(),
            tags: serde_json::json!(["x"]),
            embedded_at: None,
            created_at: "2026-04-15T00:00:00Z".into(),
            updated_at: "2026-04-15T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&a).unwrap();
        let decoded: Article = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.source_id, "https://example.com/x");
        assert_eq!(decoded.content_hash, "abc123");
    }

    #[test]
    fn test_knowledge_store_serde_has_quantizer_version() {
        let s = KnowledgeStore {
            id: "s1".into(),
            owner_id: "u1".into(),
            store_type: "personal".into(),
            name: "N".into(),
            lancedb_collection: "c".into(),
            quantizer_version: "ivf_pq_v1".into(),
            created_at: "t".into(),
            updated_at: "t".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("ivf_pq_v1"));
    }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p knowledge-nexus-agent store::models`
Expected: `test result: ok. 2 passed`.

- [ ] **Step 4: Commit**

```bash
git add src/store/models.rs
git commit -m "store: add 1.0.0 models with content_hash, source_id, quantizer_version"
```

---

## Task 5: Define SurrealQL schema in `src/store/schema.rs`

**Files:**
- Create: `src/store/schema.rs`

- [ ] **Step 1: Write the schema module**

Create `src/store/schema.rs`:
```rust
//! SurrealQL DDL for the 1.0.0 relational layer.
//!
//! The schema is SCHEMAFULL — unknown fields are rejected. Graph edges
//! (`MENTIONS`, `TAGGED`, etc.) and entity/tag tables are added in P3 and
//! are intentionally absent from this file.

pub const SCHEMA_VERSION: &str = "1.0.0-p1";

/// Returns the full SurrealQL DDL script. Idempotent: every statement uses
/// `IF NOT EXISTS` or `OVERWRITE` so re-running on an initialized DB is a
/// no-op.
pub fn ddl() -> &'static str {
    r#"
-- User
DEFINE TABLE IF NOT EXISTS user SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS username ON user TYPE string ASSERT $value != NONE;
DEFINE FIELD IF NOT EXISTS display_name ON user TYPE string;
DEFINE FIELD IF NOT EXISTS is_owner ON user TYPE bool DEFAULT false;
DEFINE FIELD IF NOT EXISTS settings ON user TYPE object DEFAULT {};
DEFINE FIELD IF NOT EXISTS created_at ON user TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at ON user TYPE string;
DEFINE INDEX IF NOT EXISTS user_username_unique ON user FIELDS username UNIQUE;
DEFINE INDEX IF NOT EXISTS user_is_owner_idx ON user FIELDS is_owner;

-- Knowledge store
DEFINE TABLE IF NOT EXISTS knowledge_store SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS owner_id ON knowledge_store TYPE string;
DEFINE FIELD IF NOT EXISTS store_type ON knowledge_store TYPE string
    ASSERT $value IN ["personal", "family", "shared"];
DEFINE FIELD IF NOT EXISTS name ON knowledge_store TYPE string;
DEFINE FIELD IF NOT EXISTS lancedb_collection ON knowledge_store TYPE string;
DEFINE FIELD IF NOT EXISTS quantizer_version ON knowledge_store TYPE string DEFAULT "ivf_pq_v1";
DEFINE FIELD IF NOT EXISTS created_at ON knowledge_store TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at ON knowledge_store TYPE string;
DEFINE INDEX IF NOT EXISTS knowledge_store_lancedb_collection_unique
    ON knowledge_store FIELDS lancedb_collection UNIQUE;
DEFINE INDEX IF NOT EXISTS knowledge_store_owner_idx
    ON knowledge_store FIELDS owner_id;

-- Article
DEFINE TABLE IF NOT EXISTS article SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS store_id ON article TYPE string;
DEFINE FIELD IF NOT EXISTS title ON article TYPE string;
DEFINE FIELD IF NOT EXISTS content ON article TYPE string;
DEFINE FIELD IF NOT EXISTS source_type ON article TYPE string DEFAULT "user";
DEFINE FIELD IF NOT EXISTS source_id ON article TYPE string DEFAULT "";
DEFINE FIELD IF NOT EXISTS content_hash ON article TYPE string DEFAULT "";
DEFINE FIELD IF NOT EXISTS tags ON article TYPE array DEFAULT [];
DEFINE FIELD IF NOT EXISTS embedded_at ON article TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at ON article TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at ON article TYPE string;
DEFINE INDEX IF NOT EXISTS article_store_idx ON article FIELDS store_id;
DEFINE INDEX IF NOT EXISTS article_hash_idx ON article FIELDS store_id, content_hash;

-- FTS: BM25 search over title + content. Analyzer tokenizes on whitespace
-- and applies ASCII + lowercase folding. SurrealDB BM25 is the FTS5 analog.
DEFINE ANALYZER IF NOT EXISTS article_ascii
    TOKENIZERS class FILTERS ascii, lowercase;
DEFINE INDEX IF NOT EXISTS article_title_search
    ON article FIELDS title SEARCH ANALYZER article_ascii BM25 HIGHLIGHTS;
DEFINE INDEX IF NOT EXISTS article_content_search
    ON article FIELDS content SEARCH ANALYZER article_ascii BM25 HIGHLIGHTS;

-- Conversation
DEFINE TABLE IF NOT EXISTS conversation SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS user_id ON conversation TYPE string;
DEFINE FIELD IF NOT EXISTS title ON conversation TYPE string DEFAULT "New Conversation";
DEFINE FIELD IF NOT EXISTS message_count ON conversation TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS created_at ON conversation TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at ON conversation TYPE string;
DEFINE INDEX IF NOT EXISTS conversation_user_idx ON conversation FIELDS user_id;

-- Message
DEFINE TABLE IF NOT EXISTS message SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS conversation_id ON message TYPE string;
DEFINE FIELD IF NOT EXISTS role ON message TYPE string
    ASSERT $value IN ["user", "assistant", "system"];
DEFINE FIELD IF NOT EXISTS content ON message TYPE string;
DEFINE FIELD IF NOT EXISTS metadata ON message TYPE object DEFAULT {};
DEFINE FIELD IF NOT EXISTS created_at ON message TYPE string;
DEFINE INDEX IF NOT EXISTS message_conversation_idx ON message FIELDS conversation_id;

-- K2K client
DEFINE TABLE IF NOT EXISTS k2k_client SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS public_key_pem ON k2k_client TYPE string;
DEFINE FIELD IF NOT EXISTS client_name ON k2k_client TYPE string DEFAULT "";
DEFINE FIELD IF NOT EXISTS registered_at ON k2k_client TYPE string;
DEFINE FIELD IF NOT EXISTS status ON k2k_client TYPE string DEFAULT "approved";
DEFINE INDEX IF NOT EXISTS k2k_client_status_idx ON k2k_client FIELDS status;

-- Federation agreement
DEFINE TABLE IF NOT EXISTS federation_agreement SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS local_store_id ON federation_agreement TYPE string;
DEFINE FIELD IF NOT EXISTS remote_node_id ON federation_agreement TYPE string;
DEFINE FIELD IF NOT EXISTS remote_endpoint ON federation_agreement TYPE string;
DEFINE FIELD IF NOT EXISTS access_type ON federation_agreement TYPE string
    ASSERT $value IN ["read", "write", "readwrite"];
DEFINE FIELD IF NOT EXISTS created_at ON federation_agreement TYPE string;
DEFINE INDEX IF NOT EXISTS federation_agreement_store_idx
    ON federation_agreement FIELDS local_store_id;

-- Discovered node
DEFINE TABLE IF NOT EXISTS discovered_node SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS host ON discovered_node TYPE string;
DEFINE FIELD IF NOT EXISTS port ON discovered_node TYPE int
    ASSERT $value >= 1 AND $value <= 65535;
DEFINE FIELD IF NOT EXISTS endpoint ON discovered_node TYPE string;
DEFINE FIELD IF NOT EXISTS capabilities ON discovered_node TYPE array DEFAULT [];
DEFINE FIELD IF NOT EXISTS last_seen ON discovered_node TYPE string;
DEFINE FIELD IF NOT EXISTS healthy ON discovered_node TYPE bool DEFAULT true;

-- Connector config
DEFINE TABLE IF NOT EXISTS connector_config SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS connector_type ON connector_config TYPE string;
DEFINE FIELD IF NOT EXISTS name ON connector_config TYPE string;
DEFINE FIELD IF NOT EXISTS config ON connector_config TYPE object DEFAULT {};
DEFINE FIELD IF NOT EXISTS store_id ON connector_config TYPE string;
DEFINE FIELD IF NOT EXISTS created_at ON connector_config TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at ON connector_config TYPE string;
DEFINE INDEX IF NOT EXISTS connector_config_store_idx
    ON connector_config FIELDS store_id;

-- Schema-version tracking table (used by `src/store/migrations.rs`).
DEFINE TABLE IF NOT EXISTS _schema_version SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS version ON _schema_version TYPE string;
DEFINE FIELD IF NOT EXISTS applied_at ON _schema_version TYPE string;
"#
}
```

- [ ] **Step 2: Compile**

Run: `cargo build`
Expected: PASS. `ddl()` is dead code but compiles.

- [ ] **Step 3: Commit**

```bash
git add src/store/schema.rs
git commit -m "store: define SurrealQL schema for 1.0.0 relational tables"
```

---

## Task 6: Implement `src/store/migrations.rs`

**Files:**
- Create: `src/store/migrations.rs`

- [ ] **Step 1: Write the migrations runner**

Create `src/store/migrations.rs`:
```rust
//! Schema version tracking for SurrealDB.
//!
//! On every `SurrealStore::open`, we run `schema::ddl()` (which is idempotent),
//! then record the version in the `_schema_version` table. Future phases
//! (P3 graph, P5 quantizer fields) will append new DDL blocks and bump
//! `SCHEMA_VERSION` — each past version's DDL stays in this file forever.

use anyhow::{Context, Result};
use surrealdb::engine::any::Any;
use surrealdb::Surreal;

use super::schema;

pub async fn run_migrations(db: &Surreal<Any>) -> Result<()> {
    db.query(schema::ddl())
        .await
        .context("Failed to apply SurrealDB DDL")?
        .check()
        .context("SurrealDB DDL returned an error")?;

    let applied_at = chrono::Utc::now().to_rfc3339();
    db.query(
        "CREATE _schema_version CONTENT { version: $version, applied_at: $applied_at }",
    )
    .bind(("version", schema::SCHEMA_VERSION))
    .bind(("applied_at", applied_at))
    .await
    .context("Failed to record schema version")?
    .check()
    .context("Schema version write returned an error")?;

    tracing::info!("SurrealDB schema at version {}", schema::SCHEMA_VERSION);
    Ok(())
}
```

- [ ] **Step 2: Compile**

Run: `cargo build`
Expected: PASS. Unused but compiles.

- [ ] **Step 3: Commit**

```bash
git add src/store/migrations.rs
git commit -m "store: add DDL runner and schema version tracking"
```

---

## Task 7: Implement `SurrealStore::open` + `open_in_memory`

**Files:**
- Modify: `src/store/mod.rs`

- [ ] **Step 1: Add the SurrealStore struct + constructors**

Append to `src/store/mod.rs` (below the trait definition, before any `#[cfg(test)]`):
```rust
use std::path::Path;
use std::sync::Arc;

use surrealdb::engine::any::{connect, Any};
use surrealdb::Surreal;

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
```

Also add the additional imports at the top of the file (right under the existing `use` block):
```rust
use anyhow::Context;
```

- [ ] **Step 2: Add `async-trait` to Cargo if not present**

Run: `cargo tree -e no-dev | grep async-trait`
Expected: a line like `async-trait v0.1.x` (it is already a dep — see `Cargo.toml` line 113 `async-trait = "0.1"`). If missing, add it under the async utilities block.

- [ ] **Step 3: Write the open test**

Add at the bottom of `src/store/mod.rs`:
```rust
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
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p knowledge-nexus-agent store::open_tests`
Expected: `test result: ok. 2 passed`.

- [ ] **Step 5: Commit**

```bash
git add src/store/mod.rs
git commit -m "store: implement SurrealStore::open and open_in_memory"
```

---

## Task 8: Implement User CRUD on `SurrealStore`

**Files:**
- Modify: `src/store/mod.rs`

- [ ] **Step 1: Start a single `#[async_trait] impl Store for SurrealStore` block**

In `src/store/mod.rs`, add below the `impl SurrealStore` block:

```rust
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

    // ... stubs for the rest so the trait impl compiles:
    async fn create_store(&self, _s: &KnowledgeStore) -> Result<()> { unimplemented!() }
    async fn get_store(&self, _id: &str) -> Result<Option<KnowledgeStore>> { unimplemented!() }
    async fn list_stores(&self) -> Result<Vec<KnowledgeStore>> { unimplemented!() }
    async fn list_stores_for_user(&self, _o: &str) -> Result<Vec<KnowledgeStore>> { unimplemented!() }
    async fn create_article(&self, _a: &Article) -> Result<()> { unimplemented!() }
    async fn get_article(&self, _id: &str) -> Result<Option<Article>> { unimplemented!() }
    async fn update_article(&self, _a: &Article) -> Result<()> { unimplemented!() }
    async fn delete_article(&self, _id: &str) -> Result<()> { unimplemented!() }
    async fn list_articles_for_store(&self, _s: &str) -> Result<Vec<Article>> { unimplemented!() }
    async fn count_articles_for_owner(&self, _o: &str) -> Result<usize> { unimplemented!() }
    async fn create_conversation(&self, _c: &Conversation) -> Result<()> { unimplemented!() }
    async fn get_conversation(&self, _id: &str) -> Result<Option<Conversation>> { unimplemented!() }
    async fn list_conversations_for_user(&self, _u: &str) -> Result<Vec<Conversation>> { unimplemented!() }
    async fn create_message(&self, _m: &Message) -> Result<()> { unimplemented!() }
    async fn list_messages_for_conversation(&self, _c: &str) -> Result<Vec<Message>> { unimplemented!() }
    async fn upsert_k2k_client(&self, _c: &K2KClient) -> Result<()> { unimplemented!() }
    async fn get_k2k_client(&self, _c: &str) -> Result<Option<K2KClient>> { unimplemented!() }
    async fn list_k2k_clients(&self) -> Result<Vec<K2KClient>> { unimplemented!() }
    async fn list_pending_k2k_clients(&self) -> Result<Vec<K2KClient>> { unimplemented!() }
    async fn update_k2k_client_status(&self, _c: &str, _s: &str) -> Result<()> { unimplemented!() }
    async fn delete_k2k_client(&self, _c: &str) -> Result<()> { unimplemented!() }
    async fn create_federation_agreement(&self, _a: &FederationAgreement) -> Result<()> { unimplemented!() }
    async fn list_federation_agreements(&self) -> Result<Vec<FederationAgreement>> { unimplemented!() }
    async fn delete_federation_agreement(&self, _id: &str) -> Result<()> { unimplemented!() }
    async fn upsert_discovered_node(&self, _n: &DiscoveredNode) -> Result<()> { unimplemented!() }
    async fn list_discovered_nodes(&self) -> Result<Vec<DiscoveredNode>> { unimplemented!() }
    async fn mark_node_unhealthy(&self, _n: &str) -> Result<()> { unimplemented!() }
    async fn delete_discovered_node(&self, _n: &str) -> Result<()> { unimplemented!() }
    async fn create_connector_config(&self, _c: &ConnectorConfig) -> Result<()> { unimplemented!() }
    async fn list_connector_configs(&self) -> Result<Vec<ConnectorConfig>> { unimplemented!() }
    async fn delete_connector_config(&self, _id: &str) -> Result<()> { unimplemented!() }
    async fn fts_search_articles(&self, _q: &str, _l: usize) -> Result<Vec<Article>> { unimplemented!() }
    async fn find_article_by_hash(&self, _s: &str, _h: &str) -> Result<Option<Article>> { unimplemented!() }
}
```

(Stubs will be replaced in tasks 9–16; each task deletes the stub it is replacing.)

- [ ] **Step 2: Write the User CRUD test**

Add to the `#[cfg(test)] mod open_tests` block (or create a new `#[cfg(test)] mod user_tests`):
```rust
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
```

- [ ] **Step 3: Run the test**

Run: `cargo test -p knowledge-nexus-agent store::user_tests`
Expected: `test result: ok. 1 passed`.

- [ ] **Step 4: Commit**

```bash
git add src/store/mod.rs
git commit -m "store: implement User CRUD on SurrealStore"
```

---

## Task 9: Implement KnowledgeStore CRUD

**Files:**
- Modify: `src/store/mod.rs`

- [ ] **Step 1: Replace the four `create_store`/`get_store`/`list_stores`/`list_stores_for_user` stubs**

In the `impl Store for SurrealStore` block:
```rust
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
        .query(
            "SELECT *, meta::id(id) AS id FROM type::thing('knowledge_store', $id)",
        )
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
```

- [ ] **Step 2: Write the store test**

Append a new `mod` block in `src/store/mod.rs`:
```rust
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
```

- [ ] **Step 3: Run**

Run: `cargo test -p knowledge-nexus-agent store::store_tests`
Expected: `test result: ok. 1 passed`.

- [ ] **Step 4: Commit**

```bash
git add src/store/mod.rs
git commit -m "store: implement KnowledgeStore CRUD"
```

---

## Task 10: Implement Article CRUD + `find_article_by_hash`

**Files:**
- Modify: `src/store/mod.rs`

- [ ] **Step 1: Replace the six Article stubs and `find_article_by_hash`**

Replace those stubs with:
```rust
async fn create_article(&self, a: &Article) -> Result<()> {
    self.db()
        .query(
            "CREATE type::thing('article', $id) CONTENT {
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
                title: $title,
                content: $content,
                source_type: $source_type,
                source_id: $source_id,
                content_hash: $content_hash,
                tags: $tags,
                embedded_at: $embedded_at,
                updated_at: $updated_at
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
    // Take the final RETURN count.
    let count: Option<i64> = resp.take(1)?;
    Ok(count.unwrap_or(0) as usize)
}

async fn find_article_by_hash(
    &self,
    store_id: &str,
    content_hash: &str,
) -> Result<Option<Article>> {
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
```

- [ ] **Step 2: Write the article test**

Append:
```rust
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
            id: "a1".into(),
            store_id: "s1".into(),
            title: "Test".into(),
            content: "Hello world".into(),
            source_type: "user".into(),
            source_id: "".into(),
            content_hash: "abc123".into(),
            tags: serde_json::json!(["test"]),
            embedded_at: None,
            created_at: ts.clone(),
            updated_at: ts.clone(),
        };
        s.create_article(&article).await.unwrap();

        let got = s.get_article("a1").await.unwrap().expect("exists");
        assert_eq!(got.title, "Test");
        assert_eq!(got.content_hash, "abc123");

        let hash_hit = s
            .find_article_by_hash("s1", "abc123")
            .await
            .unwrap()
            .expect("hash match");
        assert_eq!(hash_hit.id, "a1");

        assert!(
            s.find_article_by_hash("s1", "nope").await.unwrap().is_none()
        );

        let mut updated = got.clone();
        updated.title = "Updated".into();
        s.update_article(&updated).await.unwrap();
        assert_eq!(
            s.get_article("a1").await.unwrap().unwrap().title,
            "Updated"
        );

        assert_eq!(s.list_articles_for_store("s1").await.unwrap().len(), 1);
        assert_eq!(s.count_articles_for_owner("u1").await.unwrap(), 1);

        s.delete_article("a1").await.unwrap();
        assert!(s.get_article("a1").await.unwrap().is_none());
    }
}
```

- [ ] **Step 3: Run**

Run: `cargo test -p knowledge-nexus-agent store::article_tests`
Expected: `test result: ok. 1 passed`.

- [ ] **Step 4: Commit**

```bash
git add src/store/mod.rs
git commit -m "store: implement Article CRUD + content_hash lookup"
```

---

## Task 11: Implement Conversation + Message CRUD

**Files:**
- Modify: `src/store/mod.rs`

- [ ] **Step 1: Replace the Conversation + Message stubs**

```rust
async fn create_conversation(&self, c: &Conversation) -> Result<()> {
    self.db()
        .query(
            "CREATE type::thing('conversation', $id) CONTENT {
                user_id: $user_id,
                title: $title,
                message_count: $message_count,
                created_at: $created_at,
                updated_at: $updated_at
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
                role: $role,
                content: $content,
                metadata: $metadata,
                created_at: $created_at
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
```

- [ ] **Step 2: Write the conversation test**

Append:
```rust
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
```

- [ ] **Step 3: Run**

Run: `cargo test -p knowledge-nexus-agent store::conv_tests`
Expected: `test result: ok. 1 passed`.

- [ ] **Step 4: Commit**

```bash
git add src/store/mod.rs
git commit -m "store: implement Conversation + Message CRUD with atomic message_count bump"
```

---

## Task 12: Implement K2KClient CRUD

**Files:**
- Modify: `src/store/mod.rs`

- [ ] **Step 1: Replace the six K2KClient stubs**

```rust
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
        .query(
            "SELECT *, meta::id(id) AS client_id FROM type::thing('k2k_client', $id)",
        )
        .bind(("id", client_id.to_string()))
        .await?;
    let rows: Vec<K2KClient> = resp.take(0)?;
    Ok(rows.into_iter().next())
}

async fn list_k2k_clients(&self) -> Result<Vec<K2KClient>> {
    let mut resp = self
        .db()
        .query(
            "SELECT *, meta::id(id) AS client_id FROM k2k_client ORDER BY registered_at",
        )
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
```

**Note:** `K2KClient` uses `client_id` as its Rust field name, not `id`, so the SELECT aliases `meta::id(id)` to `client_id`. This matches the existing struct shape (see `src/store/models.rs`).

- [ ] **Step 2: Write the k2k client test**

Append:
```rust
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

        // Upsert semantics
        let mut updated = client.clone();
        updated.client_name = "Renamed".into();
        s.upsert_k2k_client(&updated).await.unwrap();
        assert_eq!(
            s.get_k2k_client("client1").await.unwrap().unwrap().client_name,
            "Renamed"
        );

        s.update_k2k_client_status("client1", "pending").await.unwrap();
        assert_eq!(s.list_pending_k2k_clients().await.unwrap().len(), 1);

        s.delete_k2k_client("client1").await.unwrap();
        assert!(s.get_k2k_client("client1").await.unwrap().is_none());
    }
}
```

- [ ] **Step 3: Run**

Run: `cargo test -p knowledge-nexus-agent store::k2k_tests`
Expected: `test result: ok. 1 passed`.

- [ ] **Step 4: Commit**

```bash
git add src/store/mod.rs
git commit -m "store: implement K2KClient CRUD"
```

---

## Task 13: Implement FederationAgreement, DiscoveredNode, ConnectorConfig

**Files:**
- Modify: `src/store/mod.rs`

These three are straightforward; bundled into one task to avoid redundant scaffolding.

- [ ] **Step 1: Replace the nine remaining stubs (3 each)**

```rust
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
        .query(
            "SELECT *, meta::id(id) AS id FROM federation_agreement ORDER BY created_at",
        )
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
                host: $host,
                port: $port,
                endpoint: $endpoint,
                capabilities: $capabilities,
                last_seen: $last_seen,
                healthy: $healthy
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
        .query(
            "SELECT *, meta::id(id) AS node_id FROM discovered_node ORDER BY last_seen DESC",
        )
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
                name: $name,
                config: $config,
                store_id: $store_id,
                created_at: $created_at,
                updated_at: $updated_at
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
        .query(
            "SELECT *, meta::id(id) AS id FROM connector_config ORDER BY created_at",
        )
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
```

The u16 `port` on `DiscoveredNode` deserializes back from SurrealDB's `int` type via serde's numeric coercion. Out-of-range values are rejected at write time by the schema `ASSERT`.

- [ ] **Step 2: Write tests**

Append:
```rust
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
            id: "cc1".into(),
            connector_type: "local_files".into(),
            name: "Docs".into(),
            config: serde_json::json!({"path": "/tmp/docs"}),
            store_id: "s1".into(),
            created_at: now(),
            updated_at: now(),
        }).await.unwrap();
        assert_eq!(s.list_connector_configs().await.unwrap().len(), 1);
        s.delete_connector_config("cc1").await.unwrap();
        assert!(s.list_connector_configs().await.unwrap().is_empty());
    }
}
```

- [ ] **Step 3: Run**

Run: `cargo test -p knowledge-nexus-agent store::federation_tests`
Expected: `test result: ok. 3 passed`.

- [ ] **Step 4: Commit**

```bash
git add src/store/mod.rs
git commit -m "store: implement FederationAgreement, DiscoveredNode, ConnectorConfig CRUD"
```

---

## Task 14: Implement FTS search on SurrealDB

**Files:**
- Modify: `src/store/mod.rs`

The `article_title_search` and `article_content_search` indexes defined in `schema.rs` use SurrealDB's built-in BM25 full-text search. SurrealQL uses the `@@` operator (aliased as `@n@` when combining analyzers) and `search::score(n)` for ranking.

- [ ] **Step 1: Replace the `fts_search_articles` stub**

```rust
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
```

- [ ] **Step 2: Write the FTS test**

Append:
```rust
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
```

- [ ] **Step 3: Run**

Run: `cargo test -p knowledge-nexus-agent store::fts_tests`
Expected: `test result: ok. 1 passed`.

If the query syntax errors out (`@0@`/`@1@` operators are analyzer-scoped and schema-dependent), fall back to this query form and re-run:
```sql
SELECT *, meta::id(id) AS id
FROM article
WHERE title @@ $q OR content @@ $q
LIMIT $limit
```
— this drops explicit BM25 ranking in favor of insertion order and is acceptable for P1 since the retrieval layer re-ranks via RRF anyway. Document the fallback in a code comment.

- [ ] **Step 4: Commit**

```bash
git add src/store/mod.rs
git commit -m "store: implement FTS search over article title + content"
```

---

## Task 15: Hook `Store` trait into existing `Database` consumers via an erased adapter

**Files:**
- Modify: `src/store/mod.rs`

The 15 callers currently hold `Arc<Database>`. To let them switch to `Arc<dyn Store>` without type gymnastics at call sites, we re-export a convenience alias here.

- [ ] **Step 1: Add the alias + helper**

Append to `src/store/mod.rs`:
```rust
/// Convenience alias used across the codebase.
pub type DynStore = dyn Store;

/// Boxed, arc'd instance. Handed into every service that used to hold
/// `Arc<Database>`.
pub type SharedStore = Arc<DynStore>;

/// Wrap a concrete `SurrealStore` into the shared trait object.
pub fn shared(store: SurrealStore) -> SharedStore {
    Arc::new(store)
}
```

- [ ] **Step 2: Compile**

Run: `cargo build`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/store/mod.rs
git commit -m "store: expose SharedStore alias for trait-object consumers"
```

---

## Task 16: Implement the content-hash helper

**Files:**
- Create: `src/store/hash.rs`
- Modify: `src/store/mod.rs` (add `pub mod hash;`)

The hash helper lives in the `store` module so both the migrator (Task 17) and P3 dedup can call it.

- [ ] **Step 1: Write the failing test first**

Create `src/store/hash.rs`:
```rust
//! Normalization + SHA-256 helper for article content_hash.
//!
//! Used by the SQLite → SurrealDB migrator and, in P3, by the dedup
//! pipeline. Keeping both users on the same function guarantees that
//! a migrated article hashes to the same value that a newly-ingested
//! one does.

use sha2::{Digest, Sha256};

/// Normalize then SHA-256 the content. Normalization:
///   - strip a leading/trailing whitespace
///   - collapse internal runs of whitespace to a single space
///   - lowercase
///
/// This produces stable hashes across minor formatting differences
/// without throwing away enough information that genuinely different
/// articles collide.
pub fn content_hash(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_was_ws = false;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            if !last_was_ws && !out.is_empty() {
                out.push(' ');
            }
            last_was_ws = true;
        } else {
            out.extend(ch.to_lowercase());
            last_was_ws = false;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }

    let digest = Sha256::digest(out.as_bytes());
    hex::encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_hash_is_stable_across_formatting() {
        let a = content_hash("Hello World");
        let b = content_hash("  hello   world  ");
        let c = content_hash("HELLO\nWORLD");
        assert_eq!(a, b);
        assert_eq!(a, c);
    }

    #[test]
    fn test_content_hash_differs_for_different_content() {
        assert_ne!(content_hash("foo"), content_hash("bar"));
    }

    #[test]
    fn test_content_hash_hex_length() {
        assert_eq!(content_hash("anything").len(), 64);
    }
}
```

Add to `src/store/mod.rs` beneath the other `pub mod` lines:
```rust
pub mod hash;
```

- [ ] **Step 2: Run**

Run: `cargo test -p knowledge-nexus-agent store::hash`
Expected: `test result: ok. 3 passed`.

- [ ] **Step 3: Commit**

```bash
git add src/store/hash.rs src/store/mod.rs
git commit -m "store: add content_hash helper for migration + dedup"
```

---

## Task 17: Implement the SQLite → SurrealDB migrator

**Files:**
- Modify: `src/migrate/mod.rs`

The migrator is a single function `migrate(from: &Path, to: &Path) -> Result<MigrationReport>`. It opens the legacy SQLite DB read-only, streams each table, and writes to a fresh `SurrealStore`. Last step writes a marker file.

- [ ] **Step 1: Scaffold the module + MigrationReport**

Replace `src/migrate/mod.rs` with:
```rust
//! One-shot migrator: 0.8 SQLite → 1.0.0 SurrealDB.
//!
//! Invoked via `knowledge-nexus-agent migrate --from sqlite --to surrealdb`.
//! Idempotent at the row level: every write is an upsert keyed on the legacy
//! primary key. A failure mid-run leaves the target DB partially populated
//! but NO `migration_completed` marker — `start` will refuse to open it,
//! and `--force` is needed to retry.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use rusqlite::{params, Connection};
use tracing::info;

use crate::store::{
    hash, Article, ConnectorConfig, Conversation, DiscoveredNode, FederationAgreement,
    K2KClient, KnowledgeStore, Message, Store, SurrealStore, User,
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

/// Returns true iff the marker file exists — i.e. a prior migration
/// completed fully against this directory.
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
        "Migrating 0.8 SQLite {:?} → 1.0.0 SurrealDB {:?}",
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

    // Mark completion last — only written if every table migrated successfully.
    std::fs::write(migration_marker_path(surreal_dir), chrono::Utc::now().to_rfc3339())
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
```

- [ ] **Step 2: Write each per-table migration function**

Append to `src/migrate/mod.rs`:
```rust
async fn migrate_users(sqlite: &Connection, store: &SurrealStore) -> Result<usize> {
    let mut stmt = sqlite.prepare(
        "SELECT id, username, display_name, is_owner, settings, created_at, updated_at FROM users",
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
    let mut n = 0usize;
    for row in rows {
        let user = row?;
        // create_user uses CREATE; we need upsert semantics for idempotency.
        upsert_user(store, &user).await?;
        n += 1;
    }
    Ok(n)
}

async fn upsert_user(store: &SurrealStore, user: &User) -> Result<()> {
    store
        .db()
        .query(
            "UPDATE type::thing('user', $id) CONTENT {
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

async fn migrate_stores(sqlite: &Connection, store: &SurrealStore) -> Result<usize> {
    let mut stmt = sqlite.prepare(
        "SELECT id, owner_id, store_type, name, lancedb_collection, created_at, updated_at FROM knowledge_stores",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(KnowledgeStore {
            id: row.get(0)?,
            owner_id: row.get(1)?,
            store_type: row.get(2)?,
            name: row.get(3)?,
            lancedb_collection: row.get(4)?,
            // Legacy DB has no column — default all migrated stores to IVF_PQ.
            quantizer_version: "ivf_pq_v1".to_string(),
            created_at: row.get(5)?,
            updated_at: row.get(6)?,
        })
    })?;
    let mut n = 0usize;
    for row in rows {
        let s = row?;
        store
            .db()
            .query(
                "UPDATE type::thing('knowledge_store', $id) CONTENT {
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
        n += 1;
    }
    Ok(n)
}

async fn migrate_articles(sqlite: &Connection, store: &SurrealStore) -> Result<usize> {
    let mut stmt = sqlite.prepare(
        "SELECT id, store_id, title, content, source_type, tags, embedded_at, created_at, updated_at FROM articles",
    )?;
    let rows = stmt.query_map([], |row| {
        let content: String = row.get(3)?;
        let content_hash = hash::content_hash(&content);
        Ok(Article {
            id: row.get(0)?,
            store_id: row.get(1)?,
            title: row.get(2)?,
            content,
            source_type: row.get(4)?,
            source_id: String::new(),
            content_hash,
            tags: serde_json::from_str(&row.get::<_, String>(5)?).unwrap_or_default(),
            embedded_at: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
        })
    })?;
    let mut n = 0usize;
    for row in rows {
        let a = row?;
        store
            .db()
            .query(
                "UPDATE type::thing('article', $id) CONTENT {
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
        n += 1;
    }
    Ok(n)
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
    let mut n = 0usize;
    for row in rows {
        let c = row?;
        store
            .db()
            .query(
                "UPDATE type::thing('conversation', $id) CONTENT {
                    user_id: $user_id,
                    title: $title,
                    message_count: $message_count,
                    created_at: $created_at,
                    updated_at: $updated_at
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
        n += 1;
    }
    Ok(n)
}

async fn migrate_messages(sqlite: &Connection, store: &SurrealStore) -> Result<usize> {
    let mut stmt = sqlite.prepare(
        "SELECT id, conversation_id, role, content, metadata, created_at FROM messages",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(Message {
            id: row.get(0)?,
            conversation_id: row.get(1)?,
            role: row.get(2)?,
            content: row.get(3)?,
            metadata: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
            created_at: row.get(5)?,
        })
    })?;
    let mut n = 0usize;
    for row in rows {
        let m = row?;
        // Messages do NOT bump message_count during migration — the legacy
        // value from `conversations.message_count` is authoritative.
        store
            .db()
            .query(
                "UPDATE type::thing('message', $id) CONTENT {
                    conversation_id: $conversation_id,
                    role: $role,
                    content: $content,
                    metadata: $metadata,
                    created_at: $created_at
                }",
            )
            .bind(("id", m.id.clone()))
            .bind(("conversation_id", m.conversation_id.clone()))
            .bind(("role", m.role.clone()))
            .bind(("content", m.content.clone()))
            .bind(("metadata", m.metadata.clone()))
            .bind(("created_at", m.created_at.clone()))
            .await?
            .check()?;
        n += 1;
    }
    Ok(n)
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
    let mut n = 0usize;
    for row in rows {
        store.upsert_k2k_client(&row?).await?;
        n += 1;
    }
    Ok(n)
}

async fn migrate_federation_agreements(sqlite: &Connection, store: &SurrealStore) -> Result<usize> {
    let mut stmt = sqlite.prepare(
        "SELECT id, local_store_id, remote_node_id, remote_endpoint, access_type, created_at
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
    let mut n = 0usize;
    for row in rows {
        let a = row?;
        store
            .db()
            .query(
                "UPDATE type::thing('federation_agreement', $id) CONTENT {
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
        n += 1;
    }
    Ok(n)
}

async fn migrate_discovered_nodes(sqlite: &Connection, store: &SurrealStore) -> Result<usize> {
    let mut stmt = sqlite.prepare(
        "SELECT node_id, host, port, endpoint, capabilities, last_seen, healthy FROM discovered_nodes",
    )?;
    let rows = stmt.query_map([], |row| {
        let port: i32 = row.get(2)?;
        let port_u16 = if (1..=65535).contains(&port) { port as u16 } else { 0 };
        Ok(DiscoveredNode {
            node_id: row.get(0)?,
            host: row.get(1)?,
            port: port_u16,
            endpoint: row.get(3)?,
            capabilities: serde_json::from_str(&row.get::<_, String>(4)?).unwrap_or_default(),
            last_seen: row.get(5)?,
            healthy: row.get::<_, i32>(6)? != 0,
        })
    })?;
    let mut n = 0usize;
    for row in rows {
        store.upsert_discovered_node(&row?).await?;
        n += 1;
    }
    Ok(n)
}

async fn migrate_connector_configs(sqlite: &Connection, store: &SurrealStore) -> Result<usize> {
    let mut stmt = sqlite.prepare(
        "SELECT id, connector_type, name, config, store_id, created_at, updated_at FROM connector_configs",
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
    let mut n = 0usize;
    for row in rows {
        let c = row?;
        store
            .db()
            .query(
                "UPDATE type::thing('connector_config', $id) CONTENT {
                    connector_type: $connector_type,
                    name: $name,
                    config: $config,
                    store_id: $store_id,
                    created_at: $created_at,
                    updated_at: $updated_at
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
        n += 1;
    }
    Ok(n)
}

// `_ = Store;` suppresses dead-code warnings when only trait-method callers
// are used. Real callers in main.rs and services keep the trait in use.
#[allow(dead_code)]
fn _assert_store_trait_used(_: &dyn Store) {}
```

Also: **make `SurrealStore::db()` `pub(crate)`** so the migrator can reach it. Open `src/store/mod.rs`, find `fn db(&self) -> &Surreal<Any>` and change it to `pub(crate) fn db(&self) -> &Surreal<Any>`.

- [ ] **Step 3: Compile**

Run: `cargo build`
Expected: PASS. If compilation fails because `db()` is private, apply the visibility change above and rerun.

- [ ] **Step 4: Commit**

```bash
git add src/migrate/mod.rs src/store/mod.rs
git commit -m "migrate: implement SQLite → SurrealDB per-table migrator with content_hash backfill"
```

---

## Task 18: Write the end-to-end migration integration test

**Files:**
- Create: `tests/migration_sqlite_to_surreal.rs`

- [ ] **Step 1: Write the failing test**

Create `tests/migration_sqlite_to_surreal.rs`:
```rust
//! End-to-end migration integration test.
//!
//! Seeds a fresh SQLite DB with representative data using the legacy
//! schema at the time of 0.8 (migrations v1–v3), runs the migrator, and
//! asserts that every record round-trips into SurrealDB with the expected
//! new-field values (content_hash backfilled, source_id empty,
//! quantizer_version = "ivf_pq_v1").

// Notes for the engineer:
// - This test exists OUTSIDE `src/`, so it imports the binary crate's
//   public surface. Everything under `knowledge_nexus_agent::` must
//   be re-exported from lib.rs OR the required items must be pub.
// - The project is a binary crate today. If you see "crate `knowledge_nexus_agent`
//   has no item `store`", add a `src/lib.rs` exposing the relevant modules.
//   See the note at the bottom of this file for the exact lib.rs content.

use std::fs;

use anyhow::Result;
use knowledge_nexus_agent::migrate::{self, is_migrated};
use knowledge_nexus_agent::store::{Store, SurrealStore};
use rusqlite::{params, Connection};
use tempfile::TempDir;

fn seed_legacy_sqlite(path: &std::path::Path) -> Result<()> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;

    // Minimal subset of legacy schema (matches src/db/migrations.rs v1-v3).
    conn.execute_batch(
        "CREATE TABLE users (
            id TEXT PRIMARY KEY, username TEXT UNIQUE, display_name TEXT,
            is_owner INTEGER, settings TEXT, created_at TEXT, updated_at TEXT
         );
         CREATE TABLE knowledge_stores (
            id TEXT PRIMARY KEY, owner_id TEXT, store_type TEXT, name TEXT,
            lancedb_collection TEXT, created_at TEXT, updated_at TEXT
         );
         CREATE TABLE articles (
            id TEXT PRIMARY KEY, store_id TEXT, title TEXT, content TEXT,
            source_type TEXT, tags TEXT, embedded_at TEXT, created_at TEXT, updated_at TEXT
         );
         CREATE TABLE conversations (
            id TEXT PRIMARY KEY, user_id TEXT, title TEXT,
            message_count INTEGER, created_at TEXT, updated_at TEXT
         );
         CREATE TABLE messages (
            id TEXT PRIMARY KEY, conversation_id TEXT, role TEXT,
            content TEXT, metadata TEXT, created_at TEXT
         );
         CREATE TABLE k2k_clients (
            client_id TEXT PRIMARY KEY, public_key_pem TEXT,
            client_name TEXT, registered_at TEXT, status TEXT
         );
         CREATE TABLE federation_agreements (
            id TEXT PRIMARY KEY, local_store_id TEXT, remote_node_id TEXT,
            remote_endpoint TEXT, access_type TEXT, created_at TEXT
         );
         CREATE TABLE discovered_nodes (
            node_id TEXT PRIMARY KEY, host TEXT, port INTEGER, endpoint TEXT,
            capabilities TEXT, last_seen TEXT, healthy INTEGER
         );
         CREATE TABLE connector_configs (
            id TEXT PRIMARY KEY, connector_type TEXT, name TEXT, config TEXT,
            store_id TEXT, created_at TEXT, updated_at TEXT
         );",
    )?;

    let ts = "2026-04-15T00:00:00Z";
    conn.execute(
        "INSERT INTO users VALUES (?,?,?,?,?,?,?)",
        params!["u1", "alice", "Alice", 1, "{}", ts, ts],
    )?;
    conn.execute(
        "INSERT INTO knowledge_stores VALUES (?,?,?,?,?,?,?)",
        params!["s1", "u1", "personal", "Notes", "store_s1", ts, ts],
    )?;
    conn.execute(
        "INSERT INTO articles VALUES (?,?,?,?,?,?,?,?,?)",
        params![
            "a1", "s1", "Rust ownership", "The borrow checker enforces rules.",
            "user", r#"["rust","ownership"]"#, ts, ts, ts
        ],
    )?;
    conn.execute(
        "INSERT INTO conversations VALUES (?,?,?,?,?,?)",
        params!["c1", "u1", "Chat", 1, ts, ts],
    )?;
    conn.execute(
        "INSERT INTO messages VALUES (?,?,?,?,?,?)",
        params!["m1", "c1", "user", "Hi", "{}", ts],
    )?;
    conn.execute(
        "INSERT INTO k2k_clients VALUES (?,?,?,?,?)",
        params!["client1", "PEM", "Tester", ts, "approved"],
    )?;
    conn.execute(
        "INSERT INTO federation_agreements VALUES (?,?,?,?,?,?)",
        params!["fa1", "s1", "node2", "http://n2/k2k", "read", ts],
    )?;
    conn.execute(
        "INSERT INTO discovered_nodes VALUES (?,?,?,?,?,?,?)",
        params!["node2", "192.168.1.20", 8765, "http://n2/k2k", "[]", ts, 1],
    )?;
    conn.execute(
        "INSERT INTO connector_configs VALUES (?,?,?,?,?,?,?)",
        params!["cc1", "local_files", "Docs", r#"{"path":"/tmp"}"#, "s1", ts, ts],
    )?;
    Ok(())
}

#[tokio::test]
async fn test_full_migration_round_trips_every_table() {
    let tmp = TempDir::new().unwrap();
    let sqlite_path = tmp.path().join("legacy.db");
    let surreal_dir = tmp.path().join("surreal");

    seed_legacy_sqlite(&sqlite_path).unwrap();

    let report = migrate::migrate(&sqlite_path, &surreal_dir, false)
        .await
        .expect("migration succeeds");
    assert_eq!(report.users, 1);
    assert_eq!(report.stores, 1);
    assert_eq!(report.articles, 1);
    assert_eq!(report.conversations, 1);
    assert_eq!(report.messages, 1);
    assert_eq!(report.k2k_clients, 1);
    assert_eq!(report.federation_agreements, 1);
    assert_eq!(report.discovered_nodes, 1);
    assert_eq!(report.connector_configs, 1);
    assert!(is_migrated(&surreal_dir));

    let store = SurrealStore::open(&surreal_dir).await.unwrap();

    // User
    let user = store.get_user("u1").await.unwrap().expect("u1");
    assert_eq!(user.username, "alice");
    assert!(user.is_owner);

    // Store (new field defaulted)
    let ks = store.get_store("s1").await.unwrap().expect("s1");
    assert_eq!(ks.quantizer_version, "ivf_pq_v1");

    // Article (content_hash backfilled, source_id empty, tags preserved)
    let article = store.get_article("a1").await.unwrap().expect("a1");
    assert_eq!(article.content_hash.len(), 64);
    assert_eq!(article.source_id, "");
    assert_eq!(
        article.tags,
        serde_json::json!(["rust", "ownership"])
    );

    // Hash lookup
    let lookup = store
        .find_article_by_hash("s1", &article.content_hash)
        .await
        .unwrap()
        .expect("hash lookup finds the article");
    assert_eq!(lookup.id, "a1");

    // Conversation preserves message_count from legacy (1), not recomputed.
    let conv = store.get_conversation("c1").await.unwrap().expect("c1");
    assert_eq!(conv.message_count, 1);

    // K2K client
    assert!(store.get_k2k_client("client1").await.unwrap().is_some());

    // Federation
    assert_eq!(store.list_federation_agreements().await.unwrap().len(), 1);
    assert_eq!(store.list_discovered_nodes().await.unwrap().len(), 1);

    // Connectors
    assert_eq!(store.list_connector_configs().await.unwrap().len(), 1);
}

#[tokio::test]
async fn test_migration_refuses_to_rerun_without_force() {
    let tmp = TempDir::new().unwrap();
    let sqlite_path = tmp.path().join("legacy.db");
    let surreal_dir = tmp.path().join("surreal");
    seed_legacy_sqlite(&sqlite_path).unwrap();

    migrate::migrate(&sqlite_path, &surreal_dir, false)
        .await
        .unwrap();

    let err = migrate::migrate(&sqlite_path, &surreal_dir, false)
        .await
        .expect_err("second run without --force should fail");
    assert!(err.to_string().contains("migration_completed"));
}

#[tokio::test]
async fn test_migration_force_reruns_idempotently() {
    let tmp = TempDir::new().unwrap();
    let sqlite_path = tmp.path().join("legacy.db");
    let surreal_dir = tmp.path().join("surreal");
    seed_legacy_sqlite(&sqlite_path).unwrap();

    migrate::migrate(&sqlite_path, &surreal_dir, false).await.unwrap();
    // Delete marker to simulate a partial prior run.
    fs::remove_file(migrate::migration_marker_path(&surreal_dir)).unwrap();

    let report = migrate::migrate(&sqlite_path, &surreal_dir, true).await.unwrap();
    assert_eq!(report.articles, 1); // Idempotent — same count.

    let store = SurrealStore::open(&surreal_dir).await.unwrap();
    assert_eq!(
        store.list_articles_for_store("s1").await.unwrap().len(),
        1,
        "upsert must not duplicate rows"
    );
}

// --- If you hit "crate has no item `store`" ---
// Create `src/lib.rs` with:
//
//     pub mod config;
//     pub mod constants;
//     pub mod migrate;
//     pub mod store;
//
// And update `src/main.rs` to use `knowledge_nexus_agent::*` instead of
// `mod` declarations that duplicate the lib. See Task 19 Step 3.
```

- [ ] **Step 2: If the test can't see `knowledge_nexus_agent::store`, add `src/lib.rs`**

The crate is a binary today. Integration tests in `tests/` only see `pub` items from a library crate with the same name. Create `src/lib.rs`:

```rust
//! Library surface for knowledge-nexus-agent.
//!
//! Keeps internal modules private to the binary while exposing the subset
//! that integration tests in `tests/` need. Expand conservatively: adding
//! a module here makes it part of the crate's semver surface.

pub mod store;
pub mod migrate;
```

Update `src/main.rs` to drop the local `mod store;` and `mod migrate;` declarations and replace in-file uses with `use knowledge_nexus_agent::store::...;` and `use knowledge_nexus_agent::migrate;`. Leave every other local `mod` declaration (`mod db;`, `mod config;`, etc.) untouched — they stay binary-private.

- [ ] **Step 3: Run the integration test**

Run: `cargo test --test migration_sqlite_to_surreal -- --nocapture`
Expected: `test result: ok. 3 passed`.

- [ ] **Step 4: Commit**

```bash
git add tests/migration_sqlite_to_surreal.rs src/lib.rs src/main.rs
git commit -m "migrate: end-to-end integration test for SQLite → SurrealDB migration"
```

---

## Task 19: Add the `Migrate` CLI subcommand

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Add the Commands variant**

In `src/main.rs`, add to the `Commands` enum (after the last existing variant but before the closing `}`):

```rust
    /// Migrate a 0.8 SQLite database into 1.0.0 SurrealDB format.
    ///
    /// One-shot. The legacy SQLite file is opened read-only and is never
    /// modified. A `migration_completed` marker is written inside the
    /// target SurrealDB directory on success.
    Migrate {
        /// Source database format. Only `sqlite` is supported.
        #[arg(long, default_value = "sqlite")]
        from: String,

        /// Target database format. Only `surrealdb` is supported.
        #[arg(long, default_value = "surrealdb")]
        to: String,

        /// Re-run even if `migration_completed` marker exists.
        #[arg(long)]
        force: bool,
    },
```

- [ ] **Step 2: Add the match arm**

Find the `match cli.command { ... }` block (around line 285). Add before the closing brace:

```rust
        Commands::Migrate { from, to, force } => {
            if from != "sqlite" {
                anyhow::bail!("--from only supports 'sqlite' in 1.0.0 (got {})", from);
            }
            if to != "surrealdb" {
                anyhow::bail!("--to only supports 'surrealdb' in 1.0.0 (got {})", to);
            }
            let sqlite_path = config::sqlite_path();
            let surreal_dir = config::data_dir().join("surreal");
            if !sqlite_path.exists() {
                anyhow::bail!(
                    "No legacy SQLite DB found at {:?}. Nothing to migrate.",
                    sqlite_path
                );
            }
            info!(
                "Starting migration: {:?} → {:?} (force={})",
                sqlite_path, surreal_dir, force
            );
            let rt = tokio::runtime::Runtime::new()?;
            let report = rt.block_on(knowledge_nexus_agent::migrate::migrate(
                &sqlite_path,
                &surreal_dir,
                force,
            ))?;
            println!(
                "Migration complete:\n  \
                 users: {}\n  stores: {}\n  articles: {}\n  conversations: {}\n  \
                 messages: {}\n  k2k_clients: {}\n  federation_agreements: {}\n  \
                 discovered_nodes: {}\n  connector_configs: {}",
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
            Ok(())
        }
```

If the surrounding match arms are sync and return `Result<()>` directly, match that style. If other arms spawn their own runtimes (`tokio::runtime::Runtime::new()`), use the same pattern — do not double-initialize a runtime.

- [ ] **Step 3: Manual CLI smoke test**

Run: `cargo run -- migrate --help`
Expected:
```
Usage: knowledge-nexus-agent migrate [OPTIONS]

Options:
      --from <FROM>  Source database format. Only `sqlite` is supported. [default: sqlite]
      --to <TO>      Target database format. Only `surrealdb` is supported. [default: surrealdb]
      --force        Re-run even if `migration_completed` marker exists.
  ...
```

Run: `cargo run -- migrate`
Expected (if no legacy SQLite exists): `Error: No legacy SQLite DB found at ...`. This is the correct failure mode.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "cli: add `migrate --from sqlite --to surrealdb` subcommand"
```

---

## Task 20: Rewire `main.rs` startup to `SurrealStore` with migration detection

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Replace the SQLite init block with a startup-flow function**

Find the `start` command handler (around the existing code near line 337–371 that calls `db::Database::open`). Replace the entire `Initialize SQLite database` + `Create default owner user` block with a new helper:

```rust
/// Open the SurrealDB store, creating the owner user on first run.
/// Refuses to start if a legacy SQLite DB exists but no migration has run.
async fn open_store_or_bail(cfg: &config::Config) -> Result<std::sync::Arc<dyn knowledge_nexus_agent::store::Store>> {
    use knowledge_nexus_agent::migrate;
    use knowledge_nexus_agent::store::{KnowledgeStore, SurrealStore, User};

    let surreal_dir = config::data_dir().join("surreal");
    let sqlite_path = config::sqlite_path();
    let surreal_exists = surreal_dir.exists()
        && surreal_dir.read_dir().map(|mut d| d.next().is_some()).unwrap_or(false);
    let migration_complete = migrate::is_migrated(&surreal_dir);
    let legacy_sqlite_exists = sqlite_path.exists();

    // Decision tree:
    //   1. SurrealDB dir exists AND marker present → normal startup.
    //   2. SurrealDB dir exists AND no marker AND legacy SQLite exists → refuse,
    //      operator must re-run migrate (with --force).
    //   3. No SurrealDB dir AND legacy SQLite exists → refuse, require migrate.
    //   4. Neither exists → fresh install.
    match (surreal_exists, migration_complete, legacy_sqlite_exists) {
        (true, true, _) => {
            info!("Opening SurrealDB at {:?}", surreal_dir);
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
            info!("No existing database — creating fresh SurrealDB at {:?}", surreal_dir);
        }
    }

    let store = SurrealStore::open(&surreal_dir).await?;

    // First-run: create default owner user + personal store if absent.
    if store.get_owner_user().await?.is_none() {
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
        store.create_user(&user).await?;
        info!("Created default owner user: {}", user.username);

        let ks = KnowledgeStore {
            id: store_id.clone(),
            owner_id: user_id.clone(),
            store_type: "personal".into(),
            name: format!("{}'s Knowledge", cfg.device.name),
            lancedb_collection: format!("store_{}", store_id),
            quantizer_version: "ivf_pq_v1".into(),
            created_at: now.clone(),
            updated_at: now,
        };
        store.create_store(&ks).await?;
        info!("Created default personal store: {}", ks.name);
    }

    let shared: std::sync::Arc<dyn knowledge_nexus_agent::store::Store> = std::sync::Arc::new(store);
    Ok(shared)
}
```

Replace the existing `let database = std::sync::Arc::new(db::Database::open(&sqlite_path)?);` and following owner-setup block with a single line (inside whatever runtime context is already set up):

```rust
let store = rt.block_on(open_store_or_bail(&cfg))?;
```

Every subsequent reference to `database` in the same function gets renamed to `store`.

If `start` is already `async fn`, use `let store = open_store_or_bail(&cfg).await?;` directly.

- [ ] **Step 2: Compile**

Run: `cargo build`
Expected: many type errors at every consumer that still expects `Arc<Database>`. Those are fixed in Task 21.

- [ ] **Step 3: Commit (broken state is OK — fixed next task)**

```bash
git add src/main.rs
git commit -m "main: open SurrealStore with migration-detection guard

Consumers still expect Arc<Database>. Rewired in the next commit."
```

---

## Task 21: Rewire every remaining `crate::db::*` consumer to `crate::store::*`

**Files (in order of dependency):**
- Modify: `src/router/mod.rs`
- Modify: `src/router/planner.rs`
- Modify: `src/k2k/server.rs`
- Modify: `src/k2k/tasks.rs`
- Modify: `src/k2k/keys.rs`
- Modify: `src/k2k/handlers.rs`
- Modify: `src/federation/agreements.rs`
- Modify: `src/federation/remote_query.rs`
- Modify: `src/knowledge/articles.rs`
- Modify: `src/knowledge/extraction.rs`
- Modify: `src/knowledge/conversations.rs`
- Modify: `src/retrieval/hybrid.rs`
- Modify: `src/discovery/registry.rs`
- Modify: `src/connectors/web_clip.rs`

**For every file:**

1. Replace `use crate::db::Database;` with `use crate::store::Store;`.
2. Replace `use crate::db::{Database, X, Y};` with `use crate::store::{Store, X, Y};`.
3. Replace `use crate::db::X;` (type-only) with `use crate::store::X;`.
4. Replace field/param types `Arc<Database>` → `Arc<dyn Store>`.
5. Replace every `.some_method()` that used to hit `Database` with `.some_method().await` — the `Store` trait is async-only.
6. Where a caller is currently synchronous (no surrounding `async fn`, no tokio runtime), mark the containing method `async`; callers bubble up until they hit main.rs.

- [ ] **Step 1: Rewire `src/router/mod.rs`**

```rust
// Before: use crate::db::Database;
use crate::store::Store;
```
Replace `Arc<Database>` → `Arc<dyn Store>` in every signature in this file.

- [ ] **Step 2: Rewire `src/router/planner.rs`**

```rust
// Before: use crate::db::{Database, KnowledgeStore};
use crate::store::{KnowledgeStore, Store};
```
Change `db: Arc<Database>` → `db: Arc<dyn Store>` and add `.await?` to every call. If any `pub fn` becomes async, update call sites transitively (mostly in `router/mod.rs`, already async-aware).

Update the `#[cfg(test)] mod tests` block: replace `use crate::db::{Database, KnowledgeStore, User};` with `use crate::store::{KnowledgeStore, Store, SurrealStore, User};`, and replace `Database::open_in_memory()?` with `SurrealStore::open_in_memory().await?`. Mark `fn setup_db` `async fn` and wrap the test function in `#[tokio::test]`.

- [ ] **Step 3: Rewire `src/k2k/server.rs`**

`crate::db::Database` → `crate::store::Store`; `Arc<Database>` → `Arc<dyn Store>`. The axum handlers are already async — add `.await` where needed.

- [ ] **Step 4: Rewire `src/k2k/tasks.rs`**

Same pattern. `db: Option<Arc<Database>>` → `db: Option<Arc<dyn Store>>`.

- [ ] **Step 5: Rewire `src/k2k/keys.rs`**

Same pattern. `crate::db::K2KClient` (used at lines 70, 154) → `crate::store::K2KClient`.

- [ ] **Step 6: Rewire `src/k2k/handlers.rs`**

Has `use crate::db;` (line 22). Replace with `use crate::store as db;` as a temporary alias — this minimizes churn inside handlers where `db::Article`, `db::Message` etc. appear often. Or replace explicitly if there are few references — check with:

Run: `grep -n "db::" src/k2k/handlers.rs | wc -l`
If the count is small (<10), replace each `db::X` with `crate::store::X`. Otherwise use the alias.

- [ ] **Step 7: Rewire `src/federation/agreements.rs`**

`crate::db::{Database, FederationAgreement}` → `crate::store::{FederationAgreement, Store}`. Field type `Arc<Database>` → `Arc<dyn Store>`. Add `.await` to DB calls.

- [ ] **Step 8: Rewire `src/federation/remote_query.rs`**

Only type import: `use crate::db::DiscoveredNode;` → `use crate::store::DiscoveredNode;`. No logic changes.

- [ ] **Step 9: Rewire `src/knowledge/articles.rs`**

Same pattern. Most call sites are already `async`.

- [ ] **Step 10: Rewire `src/knowledge/extraction.rs`**

Same pattern. `crate::db::{Article, Database, Message}` → `crate::store::{Article, Message, Store}`.

- [ ] **Step 11: Rewire `src/knowledge/conversations.rs`**

Same pattern.

- [ ] **Step 12: Rewire `src/retrieval/hybrid.rs`**

Same pattern. The file calls `db.fts_search_articles(...)` — swap to `store.fts_search_articles(...).await?`. The hybrid searcher is already async, so this is mechanical.

- [ ] **Step 13: Rewire `src/discovery/registry.rs`**

Same pattern.

- [ ] **Step 14: Rewire `src/connectors/web_clip.rs`**

Same pattern.

- [ ] **Step 15: Compile**

Run: `cargo build`
Expected: PASS. Every error points to a missing `.await` or a stale `Database` reference — fix and re-run until green.

- [ ] **Step 16: Run the full test suite**

Run: `cargo test -p knowledge-nexus-agent`
Expected: every test passes. SurrealDB-backed tests in `store::` pass; the legacy `db::` tests (still present in `src/db/repository.rs`) also pass because the old code is still alive.

- [ ] **Step 17: Commit**

```bash
git add src/
git commit -m "rewire: migrate all Database consumers to Store trait"
```

---

## Task 22: Delete the legacy `src/db/` module

**Files:**
- Delete: `src/db/mod.rs`, `src/db/migrations.rs`, `src/db/models.rs`, `src/db/repository.rs`
- Modify: `src/main.rs` (remove `mod db;`)

- [ ] **Step 1: Confirm no consumers remain**

Run:
```bash
grep -rn "crate::db::" src/ tests/ || echo "no consumers"
grep -rn "mod db;" src/ || echo "no mod decl"
```
Expected: `no consumers` and `no mod decl` — or a single `mod db;` line in `src/main.rs` only.

If any `grep` still returns results, go back to Task 21 Step N for that file and finish the rewrite before deleting.

- [ ] **Step 2: Delete the directory + remove the module declaration**

```bash
rm -r src/db
```

Edit `src/main.rs`, remove the `mod db;` line.

- [ ] **Step 3: Compile + test**

Run: `cargo build && cargo test -p knowledge-nexus-agent`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add -u
git commit -m "db: remove legacy SQLite repository layer"
```

**Note:** `rusqlite` stays in `Cargo.toml` because the migrator in `src/migrate/mod.rs` still uses it to read legacy DBs. Removing `rusqlite` is a separate maintenance PR after 1.0.0 releases (per spec §9 and §12).

---

## Task 23: Final smoke test — CLI commands work end-to-end

**Files:** none

- [ ] **Step 1: Fresh install smoke test**

```bash
# Use a scratch data dir so we don't touch the user's real config.
export XDG_DATA_HOME=/tmp/kn-smoke-data
export XDG_CONFIG_HOME=/tmp/kn-smoke-cfg
rm -rf /tmp/kn-smoke-*
cargo run --release -- init --hub-url ws://localhost:9999
cargo run --release -- status
```
Expected: `init` creates config. `status` prints "stopped" (no running agent).

- [ ] **Step 2: Check that `start` opens the SurrealDB**

```bash
cargo run --release -- start --no-tray --foreground &
START_PID=$!
sleep 5
ls /tmp/kn-smoke-data/knowledge-nexus-agent/surreal/
kill $START_PID
```
Expected: the `surreal/` dir exists and is populated after startup. The first-run owner user + store were created. No migration_completed marker yet (only present post-migrate).

Actually, on a **fresh install** there IS no prior migration, so the marker should NOT exist. `open_store_or_bail`'s case (4) "neither exists → fresh install" applies. Re-check the decision tree in Task 20 if `start` fails.

- [ ] **Step 3: Migration smoke test with a seeded legacy DB**

```bash
rm -rf /tmp/kn-smoke-*
# Borrow the integration-test fixture to seed a representative legacy DB:
cargo test --test migration_sqlite_to_surreal --no-run
# Manually reproduce what the test does — or just re-run:
cargo test --test migration_sqlite_to_surreal test_full_migration -- --nocapture
```
Expected: test passes end-to-end.

- [ ] **Step 4: Verify the CLI help is clean**

```bash
cargo run -- --help
```
Expected: `migrate` appears in the subcommand list with a proper description.

- [ ] **Step 5: Open the PR**

```bash
git push -u origin feat/p1-data-layer-migration
gh pr create \
  --title "P1: data layer migration from SQLite to SurrealDB" \
  --body "$(cat <<'EOF'
## Summary
- Replaces SQLite + FTS5 with an embedded SurrealDB (`kv-surrealkv` backend) behind a new `Store` trait.
- Adds `migrate --from sqlite --to surrealdb` for one-shot conversion of 0.8 databases. Idempotent at the row level; marker file prevents accidental reruns.
- New `Article.content_hash` and `Article.source_id` fields, and `KnowledgeStore.quantizer_version`, added now so P2/P3 don't need another schema migration.
- Graph edges, entity extraction, and dedup are NOT in this PR — they are P3.
- `rusqlite` stays a dep to support the migration window (spec §9); removed in a follow-up after 1.0.0 ships.

## Test plan
- [ ] `cargo test -p knowledge-nexus-agent` green (unit tests)
- [ ] `cargo test --test migration_sqlite_to_surreal` green (migration integration)
- [ ] Manual: `migrate --help` works and the command rejects a non-existent legacy DB cleanly
- [ ] Manual: `start` on a fresh data dir creates an owner user + personal store
- [ ] Manual: `start` on a dir with legacy SQLite but no SurrealDB errors with "Run migrate" guidance

Spec: `docs/superpowers/specs/2026-04-15-graph-and-quantizer-integration-design.md` §5.1, §6.1, §6.6, §9
Plan: `docs/superpowers/plans/2026-04-15-p1-data-layer-migration.md`
EOF
)"
```

---

## Self-review (from writing-plans skill)

**Spec coverage audit:**

- Spec §5.1 (SurrealDB tables) — Task 5 (schema DDL), Tasks 7–14 (CRUD). ✅
- Spec §5.2 (edges) — **Out of scope for P1**, deferred to P3. Documented at the top.
- Spec §5.3 (LanceDB additions) — `quantizer_version` metadata field is **P2** (schema only); P1 adds the column on `knowledge_store` but no dispatch logic. ✅
- Spec §6.1 (module reshape) — `src/store/` created (Tasks 3–6, 15, 16); `src/db/` deleted (Task 22). ✅
- Spec §6.6 (Store trait) — Task 3 defines the trait; Tasks 7–14 implement. ✅
- Spec §9 (migration strategy) — Tasks 17–19 implement; marker file, `--force`, row-level upsert idempotency all covered. ✅
- Spec §13 gates 1, 2, 6 — Tasks 18 (integration test), 23 (manual smoke), 21 (existing tests still pass). ✅ (Gates 3–5, 7–13 are P3/P4/P5.)

**Placeholder scan:** no "TBD", "similar to task N", or vague "add error handling" steps found. Every code block contains the actual text to paste.

**Type consistency:** `Store` trait method names in Task 3 match the `impl Store for SurrealStore` method names in Tasks 7–14. `Arc<dyn Store>` is used uniformly. `find_article_by_hash` has the same signature in the trait definition, the impl, and the migration test.

**Two watch-outs flagged for executor:**
1. **SurrealQL syntax** — if `@0@`/`@1@` FTS operators don't parse on the installed surrealdb crate version, Task 14 Step 3 specifies a documented fallback (drop ranking, rely on RRF in the retrieval layer).
2. **Binary vs library crate** — if integration tests complain about missing `knowledge_nexus_agent::store`, Task 18 Step 2 creates `src/lib.rs`. This is the standard Rust pattern for bin-crates that want test-visible internals.

---

## Execution handoff

Plan saved to `docs/superpowers/plans/2026-04-15-p1-data-layer-migration.md`.

Two execution options:

**1. Subagent-Driven (recommended)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.

**2. Inline Execution** — Execute tasks in this session using executing-plans, batch execution with checkpoints.

Which approach?
