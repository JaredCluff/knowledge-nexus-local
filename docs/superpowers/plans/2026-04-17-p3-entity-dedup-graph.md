# P3: Entity Extraction, Dedup, and Knowledge Graph -- Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add LLM-based entity extraction via Ollama, content dedup with a human review queue, and a SurrealDB knowledge graph (entities, tags, MENTIONS/TAGGED/RELATED_TO edges) that turns articles into a connected knowledge graph.

**Architecture:** A new `EntityExtractor` struct calls the local Ollama `/api/generate` endpoint to extract structured entities from article content. Extracted entities are upserted into an `entity` table and linked to articles via `MENTIONS` edges. Tags are migrated from the article JSON array into a `tag` table with `TAGGED` edges. Article-to-article `RELATED_TO` edges are computed eagerly at ingest based on shared entity overlap (Jaccard similarity). Content dedup runs at `ArticleService.create()` via `content_hash` lookup, queuing duplicates for human review. All graph operations use SurrealDB's `RELATE` statement and edge-table queries.

**Tech Stack:** Rust, SurrealDB 2.x (RELATE, edge tables), reqwest (Ollama HTTP), serde_json, tokio, async-trait, chrono, tracing

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `src/knowledge/entity_extractor.rs` | `EntityExtractor` struct, Ollama HTTP client, prompt construction, JSON response parsing, `entity_id()` and `slugify()` helpers |
| Modify | `src/knowledge/mod.rs` | Add `pub mod entity_extractor;` export |
| Modify | `src/knowledge/articles.rs` | Wire extraction into `embed_article()`, dedup check in `create()`, tag edge creation in `create()` |
| Modify | `src/store/schema.rs` | `entity`, `tag`, `dedup_queue` DDL; `tagged`, `mentions`, `related_to` edge table schemas; bump to `1.0.0-p3` |
| Modify | `src/store/models.rs` | `Entity`, `Tag`, `DedupQueueEntry`, `MentionsEdge`, `RelatedToEdge` structs |
| Modify | `src/store/mod.rs` | CRUD for entities, tags, dedup queue; edge creation/query; graph traversal helpers on `Store` trait + `SurrealStore` impl |
| Modify | `src/store/migrations.rs` | P3 data migration: read articles with tags, create tag records + TAGGED edges, schema version bump |
| Modify | `src/main.rs` | `dedup-review` and `extract-entities` CLI subcommands |
| Modify | `src/config/mod.rs` | `[extraction]` config section with `ExtractionConfig` struct |

---

### Task 1: Schema and models

**Files:**
- Modify: `src/store/schema.rs`
- Modify: `src/store/models.rs`

- [ ] **Step 1: Write the failing test**

Add to `src/store/models.rs` in the existing `tests` module:

```rust
    #[test]
    fn test_entity_serde_round_trip() {
        let e = Entity {
            id: "tool:rust".into(),
            name: "Rust".into(),
            entity_type: "tool".into(),
            description: Some("Systems programming language".into()),
            store_id: "s1".into(),
            mention_count: 3,
            created_at: "2026-04-17T00:00:00Z".into(),
            updated_at: "2026-04-17T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        let decoded: Entity = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, "tool:rust");
        assert_eq!(decoded.entity_type, "tool");
        assert_eq!(decoded.description, Some("Systems programming language".into()));
        assert_eq!(decoded.mention_count, 3);
    }

    #[test]
    fn test_tag_serde_round_trip() {
        let t = Tag {
            id: "machine-learning".into(),
            name: "Machine Learning".into(),
            store_id: "s1".into(),
            created_at: "2026-04-17T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let decoded: Tag = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, "machine-learning");
        assert_eq!(decoded.name, "Machine Learning");
    }

    #[test]
    fn test_dedup_queue_entry_serde_round_trip() {
        let d = DedupQueueEntry {
            id: "dq1".into(),
            store_id: "s1".into(),
            incoming_title: "Duplicate Article".into(),
            incoming_content: "Content here".into(),
            incoming_source_type: "user".into(),
            incoming_source_id: None,
            matched_article_id: "a1".into(),
            content_hash: "abc123".into(),
            status: "pending".into(),
            created_at: "2026-04-17T00:00:00Z".into(),
            resolved_at: None,
        };
        let json = serde_json::to_string(&d).unwrap();
        let decoded: DedupQueueEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.status, "pending");
        assert!(decoded.resolved_at.is_none());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib store::models::tests -- 2>&1 | head -20`
Expected: FAIL -- `Entity`, `Tag`, `DedupQueueEntry` do not exist yet.

- [ ] **Step 3: Add model structs**

Add to `src/store/models.rs`, after the `ConnectorConfig` struct:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
    pub name: String,
    pub entity_type: String,
    pub description: Option<String>,
    pub store_id: String,
    pub mention_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    pub id: String,
    pub name: String,
    pub store_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DedupQueueEntry {
    pub id: String,
    pub store_id: String,
    pub incoming_title: String,
    pub incoming_content: String,
    pub incoming_source_type: String,
    pub incoming_source_id: Option<String>,
    pub matched_article_id: String,
    pub content_hash: String,
    pub status: String,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

/// Row returned when querying a MENTIONS edge with entity fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MentionsEdge {
    pub article_id: String,
    pub entity_id: String,
    pub excerpt: String,
    pub confidence: f64,
    pub created_at: String,
}

/// Row returned when querying a RELATED_TO edge between two articles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedToEdge {
    pub from_article_id: String,
    pub to_article_id: String,
    pub shared_entity_count: i64,
    pub strength: f64,
    pub created_at: String,
    pub updated_at: String,
}
```

- [ ] **Step 4: Update schema.rs**

In `src/store/schema.rs`, change the version constant:

```rust
pub const SCHEMA_VERSION: &str = "1.0.0-p3";
```

Append the following DDL blocks inside the `r#"..."#` string, after the `_schema_version` table and before the closing `"#`:

```sql
-- Entity (P3)
DEFINE TABLE IF NOT EXISTS entity SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS name ON entity TYPE string;
DEFINE FIELD IF NOT EXISTS entity_type ON entity TYPE string
    ASSERT $value IN ["topic", "person", "project", "tool", "concept", "reference"];
DEFINE FIELD IF NOT EXISTS description ON entity TYPE option<string>;
DEFINE FIELD IF NOT EXISTS store_id ON entity TYPE string;
DEFINE FIELD IF NOT EXISTS mention_count ON entity TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS created_at ON entity TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at ON entity TYPE string;
DEFINE INDEX IF NOT EXISTS entity_store_name_type_unique
    ON entity FIELDS store_id, name, entity_type UNIQUE;
DEFINE INDEX IF NOT EXISTS entity_store_idx ON entity FIELDS store_id;

-- Tag (P3)
DEFINE TABLE IF NOT EXISTS tag SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS name ON tag TYPE string;
DEFINE FIELD IF NOT EXISTS store_id ON tag TYPE string;
DEFINE FIELD IF NOT EXISTS created_at ON tag TYPE string;
DEFINE INDEX IF NOT EXISTS tag_store_name_unique ON tag FIELDS store_id, name UNIQUE;

-- Dedup queue (P3)
DEFINE TABLE IF NOT EXISTS dedup_queue SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS store_id ON dedup_queue TYPE string;
DEFINE FIELD IF NOT EXISTS incoming_title ON dedup_queue TYPE string;
DEFINE FIELD IF NOT EXISTS incoming_content ON dedup_queue TYPE string;
DEFINE FIELD IF NOT EXISTS incoming_source_type ON dedup_queue TYPE string;
DEFINE FIELD IF NOT EXISTS incoming_source_id ON dedup_queue TYPE option<string>;
DEFINE FIELD IF NOT EXISTS matched_article_id ON dedup_queue TYPE string;
DEFINE FIELD IF NOT EXISTS content_hash ON dedup_queue TYPE string;
DEFINE FIELD IF NOT EXISTS status ON dedup_queue TYPE string DEFAULT "pending"
    ASSERT $value IN ["pending", "rejected", "merged"];
DEFINE FIELD IF NOT EXISTS created_at ON dedup_queue TYPE string;
DEFINE FIELD IF NOT EXISTS resolved_at ON dedup_queue TYPE option<string>;
DEFINE INDEX IF NOT EXISTS dedup_queue_status_idx ON dedup_queue FIELDS status;
DEFINE INDEX IF NOT EXISTS dedup_queue_store_idx ON dedup_queue FIELDS store_id;

-- TAGGED edge table (P3): article -> tag
DEFINE TABLE IF NOT EXISTS tagged SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS in ON tagged TYPE record<article>;
DEFINE FIELD IF NOT EXISTS out ON tagged TYPE record<tag>;
DEFINE FIELD IF NOT EXISTS created_at ON tagged TYPE string;
DEFINE INDEX IF NOT EXISTS tagged_unique ON tagged FIELDS in, out UNIQUE;

-- MENTIONS edge table (P3): article -> entity
DEFINE TABLE IF NOT EXISTS mentions SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS in ON mentions TYPE record<article>;
DEFINE FIELD IF NOT EXISTS out ON mentions TYPE record<entity>;
DEFINE FIELD IF NOT EXISTS excerpt ON mentions TYPE string;
DEFINE FIELD IF NOT EXISTS confidence ON mentions TYPE float DEFAULT 0.0;
DEFINE FIELD IF NOT EXISTS created_at ON mentions TYPE string;
DEFINE INDEX IF NOT EXISTS mentions_unique ON mentions FIELDS in, out UNIQUE;

-- RELATED_TO edge table (P3): article -> article
DEFINE TABLE IF NOT EXISTS related_to SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS in ON related_to TYPE record<article>;
DEFINE FIELD IF NOT EXISTS out ON related_to TYPE record<article>;
DEFINE FIELD IF NOT EXISTS shared_entity_count ON related_to TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS strength ON related_to TYPE float DEFAULT 0.0;
DEFINE FIELD IF NOT EXISTS created_at ON related_to TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at ON related_to TYPE string;
DEFINE INDEX IF NOT EXISTS related_to_unique ON related_to FIELDS in, out UNIQUE;
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test --lib store::models::tests -- 2>&1 | tail -5`
Expected: PASS -- all three serde round-trip tests pass.

- [ ] **Step 6: Verify schema applies cleanly**

Run: `cargo test --lib store::open_tests -- 2>&1 | tail -5`
Expected: PASS -- `open_in_memory` and `open_on_disk` tests pass, proving the DDL is valid SurrealQL.

- [ ] **Step 7: Commit**

```bash
git add src/store/schema.rs src/store/models.rs
git commit -m "feat(store): add P3 schema (entity, tag, dedup_queue, edge tables) and model structs"
```

---

### Task 2: Store trait -- entity CRUD

**Files:**
- Modify: `src/store/mod.rs`

- [ ] **Step 1: Write the failing test**

Add a new test module at the bottom of `src/store/mod.rs`:

```rust
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
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib store::entity_tests -- 2>&1 | head -20`
Expected: FAIL -- `create_entity`, `get_entity`, `list_entities_for_store`, `upsert_entity` not defined on trait.

- [ ] **Step 3: Add entity methods to Store trait**

In `src/store/mod.rs`, add to the `Store` trait after the `find_article_by_hash` method:

```rust
    // Entities (P3)
    async fn create_entity(&self, entity: &Entity) -> Result<()>;
    async fn get_entity(&self, id: &str) -> Result<Option<Entity>>;
    async fn list_entities_for_store(&self, store_id: &str) -> Result<Vec<Entity>>;
    async fn upsert_entity(&self, entity: &Entity) -> Result<()>;
```

- [ ] **Step 4: Implement entity methods on SurrealStore**

Add to the `impl Store for SurrealStore` block, after the `fts_search_articles` impl:

```rust
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
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test --lib store::entity_tests -- 2>&1 | tail -5`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/store/mod.rs
git commit -m "feat(store): add entity CRUD methods to Store trait and SurrealStore"
```

---

### Task 3: Store trait -- tag CRUD

**Files:**
- Modify: `src/store/mod.rs`

- [ ] **Step 1: Write the failing test**

Add a new test module at the bottom of `src/store/mod.rs`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib store::tag_tests -- 2>&1 | head -20`
Expected: FAIL -- methods not defined.

- [ ] **Step 3: Add tag methods to Store trait**

Add to the `Store` trait after the entity methods:

```rust
    // Tags (P3)
    async fn create_tag(&self, tag: &Tag) -> Result<()>;
    async fn list_tags_for_store(&self, store_id: &str) -> Result<Vec<Tag>>;
    async fn upsert_tag(&self, tag: &Tag) -> Result<()>;
```

- [ ] **Step 4: Implement tag methods on SurrealStore**

Add to the `impl Store for SurrealStore` block:

```rust
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
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test --lib store::tag_tests -- 2>&1 | tail -5`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/store/mod.rs
git commit -m "feat(store): add tag CRUD methods to Store trait and SurrealStore"
```

---

### Task 4: Store trait -- dedup queue CRUD

**Files:**
- Modify: `src/store/mod.rs`

- [ ] **Step 1: Write the failing test**

Add a new test module at the bottom of `src/store/mod.rs`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib store::dedup_tests -- 2>&1 | head -20`
Expected: FAIL -- methods not defined.

- [ ] **Step 3: Add dedup queue methods to Store trait**

Add to the `Store` trait after the tag methods:

```rust
    // Dedup queue (P3)
    async fn create_dedup_entry(&self, entry: &DedupQueueEntry) -> Result<()>;
    async fn list_pending_dedup(&self, store_id: &str) -> Result<Vec<DedupQueueEntry>>;
    async fn get_dedup_entry(&self, id: &str) -> Result<Option<DedupQueueEntry>>;
    async fn resolve_dedup_entry(&self, id: &str, status: &str) -> Result<()>;
```

- [ ] **Step 4: Implement dedup queue methods on SurrealStore**

Add to the `impl Store for SurrealStore` block:

```rust
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
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test --lib store::dedup_tests -- 2>&1 | tail -5`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/store/mod.rs
git commit -m "feat(store): add dedup queue CRUD methods to Store trait and SurrealStore"
```

---

### Task 5: Store trait -- graph edge methods

**Files:**
- Modify: `src/store/mod.rs`

- [ ] **Step 1: Write the failing test**

Add a new test module at the bottom of `src/store/mod.rs`:

```rust
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
        let s = fixture().await;
        s.create_or_update_related_to_edge("a1", "a2", 1, 0.5).await.unwrap();

        let related = s.list_related_articles("a1").await.unwrap();
        assert_eq!(related.len(), 1);
        assert_eq!(related[0].id, "a2");

        // Update strength
        s.create_or_update_related_to_edge("a1", "a2", 2, 0.8).await.unwrap();
        // Should still be 1 edge, not 2
        let related = s.list_related_articles("a1").await.unwrap();
        assert_eq!(related.len(), 1);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib store::graph_edge_tests -- 2>&1 | head -20`
Expected: FAIL -- methods not defined.

- [ ] **Step 3: Add graph edge methods to Store trait**

Add to the `Store` trait after the dedup queue methods:

```rust
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
```

- [ ] **Step 4: Implement graph edge methods on SurrealStore**

Add to the `impl Store for SurrealStore` block:

```rust
    // Graph edge methods (P3)
    async fn create_mentions_edge(
        &self, article_id: &str, entity_id: &str, excerpt: &str, confidence: f64,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        self.db()
            .query(
                "RELATE type::thing('article', $article_id)->mentions->type::thing('entity', $entity_id) CONTENT {
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
                "RELATE type::thing('article', $article_id)->tagged->type::thing('tag', $tag_id) CONTENT {
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
                "DELETE FROM related_to WHERE in = type::thing('article', $from_id) AND out = type::thing('article', $to_id);
                 RELATE type::thing('article', $from_id)->related_to->type::thing('article', $to_id) CONTENT {
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
        let mut resp = self
            .db()
            .query(
                "SELECT *, meta::id(id) AS id FROM article
                 WHERE id IN (
                    SELECT VALUE out FROM related_to
                    WHERE in = type::thing('article', $article_id)
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
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test --lib store::graph_edge_tests -- 2>&1 | tail -5`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/store/mod.rs
git commit -m "feat(store): add graph edge methods (mentions, tagged, related_to) to Store trait"
```

---

### Task 6: ExtractionConfig

**Files:**
- Modify: `src/config/mod.rs`

- [ ] **Step 1: Add ExtractionConfig struct**

Add after the `WebSearchConfig` impl block in `src/config/mod.rs`:

```rust
/// Entity extraction configuration (P3)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionConfig {
    /// Enable LLM-based entity extraction during article ingest
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Ollama API base URL
    #[serde(default = "default_ollama_url")]
    pub ollama_url: String,

    /// Ollama model name for entity extraction
    #[serde(default = "default_extraction_model")]
    pub model: String,
}

fn default_ollama_url() -> String {
    "http://localhost:11434".to_string()
}

fn default_extraction_model() -> String {
    "gemma4:e4b".to_string()
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ollama_url: default_ollama_url(),
            model: default_extraction_model(),
        }
    }
}
```

- [ ] **Step 2: Add extraction field to Config struct**

In the `Config` struct, add after the `web_search` field:

```rust
    /// Entity extraction settings (P3)
    #[serde(default)]
    pub extraction: ExtractionConfig,
```

- [ ] **Step 3: Add to Config::default()**

In the `impl Default for Config` block, add after `web_search: WebSearchConfig::default(),`:

```rust
            extraction: ExtractionConfig::default(),
```

- [ ] **Step 4: Add env var override in load_config()**

In the `load_config()` function, after the web search env var block, add:

```rust
    // Extraction config overrides
    if let Some(url) = first_env(&["KN_OLLAMA_URL"]) {
        config.extraction.ollama_url = url;
    }
    if let Some(model) = first_env(&["KN_EXTRACTION_MODEL"]) {
        config.extraction.model = model;
    }
    if let Some(enabled) = first_env(&["KN_EXTRACTION_ENABLED"]) {
        if let Ok(v) = enabled.parse::<bool>() {
            config.extraction.enabled = v;
        }
    }
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check 2>&1 | tail -5`
Expected: no errors

- [ ] **Step 6: Commit**

```bash
git add src/config/mod.rs
git commit -m "feat(config): add [extraction] config section for Ollama entity extraction"
```

---

### Task 7: EntityExtractor

**Files:**
- Create: `src/knowledge/entity_extractor.rs`
- Modify: `src/knowledge/mod.rs`

- [ ] **Step 1: Write the failing test**

Create `src/knowledge/entity_extractor.rs` with tests first:

```rust
//! LLM-based entity extraction via local Ollama.
//!
//! Calls the Ollama `/api/generate` endpoint with a structured prompt that
//! constrains the model to return a JSON array of entities. Each entity has
//! a name, type, description, confidence, and excerpt.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::ExtractionConfig;

/// A single entity extracted from article text by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedEntity {
    pub name: String,
    pub entity_type: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_confidence")]
    pub confidence: f64,
    #[serde(default)]
    pub excerpt: Option<String>,
}

fn default_confidence() -> f64 {
    0.5
}

/// Performs entity extraction against a local Ollama instance.
pub struct EntityExtractor {
    config: ExtractionConfig,
    client: reqwest::Client,
}

/// Generate a deterministic entity ID from type and name.
///
/// Format: `{entity_type}:{slug}` where slug is the name lowercased with
/// non-alphanumeric characters replaced by hyphens, with consecutive hyphens
/// collapsed and leading/trailing hyphens stripped.
pub fn entity_id(entity_type: &str, name: &str) -> String {
    let slug = slugify(name);
    format!("{}:{}", entity_type, slug)
}

/// Slugify a string: lowercase, replace non-alphanumeric with hyphens,
/// collapse consecutive hyphens, strip leading/trailing hyphens.
pub fn slugify(s: &str) -> String {
    let mut slug = String::with_capacity(s.len());
    let mut last_was_hyphen = true; // prevents leading hyphen
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            slug.extend(ch.to_lowercase());
            last_was_hyphen = false;
        } else if !last_was_hyphen {
            slug.push('-');
            last_was_hyphen = true;
        }
    }
    // Strip trailing hyphen
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

const VALID_ENTITY_TYPES: &[&str] = &["topic", "person", "project", "tool", "concept", "reference"];

/// Ollama /api/generate request body
#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    format: String,
    stream: bool,
}

/// Ollama /api/generate response body
#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
}

/// Wrapper for the JSON array the LLM returns.
#[derive(Deserialize)]
struct ExtractionResult {
    entities: Vec<ExtractedEntity>,
}

impl EntityExtractor {
    pub fn new(config: ExtractionConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("Failed to build reqwest client");
        Self { config, client }
    }

    /// Build the extraction prompt for a given article.
    fn build_prompt(title: &str, content: &str) -> String {
        format!(
            r#"You are a knowledge graph entity extractor. Analyze the following article and extract all notable entities.

For each entity, provide:
- "name": The canonical name of the entity
- "entity_type": One of: "topic", "person", "project", "tool", "concept", "reference"
- "description": A brief 1-sentence description
- "confidence": A float between 0.0 and 1.0 indicating your confidence
- "excerpt": A short text snippet from the article where this entity is mentioned

Entity type guidelines:
- "topic": A subject area or field (e.g., "Machine Learning", "Distributed Systems")
- "person": A named individual (e.g., "Linus Torvalds")
- "project": A software project or initiative (e.g., "Linux Kernel", "Kubernetes")
- "tool": A specific tool, library, language, or framework (e.g., "Rust", "PostgreSQL")
- "concept": An abstract idea or methodology (e.g., "Borrow Checking", "Event Sourcing")
- "reference": A book, paper, URL, or external resource (e.g., "The Rust Programming Language book")

Return a JSON object with a single key "entities" containing an array of entity objects. Only include entities you are at least 50% confident about. Limit to the 20 most important entities.

Title: {title}

Content:
{content}"#,
            title = title,
            content = content,
        )
    }

    /// Extract entities from article text. Returns an empty vec on any error
    /// (Ollama down, parse failure, etc.) — caller should log and continue.
    pub async fn extract(&self, title: &str, content: &str) -> Result<Vec<ExtractedEntity>> {
        if !self.config.enabled {
            return Ok(vec![]);
        }

        let prompt = Self::build_prompt(title, content);
        let url = format!("{}/api/generate", self.config.ollama_url);

        let request = OllamaRequest {
            model: self.config.model.clone(),
            prompt,
            format: "json".into(),
            stream: false,
        };

        let resp = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Failed to reach Ollama")?;

        if !resp.status().is_success() {
            anyhow::bail!("Ollama returned HTTP {}", resp.status());
        }

        let ollama_resp: OllamaResponse = resp
            .json()
            .await
            .context("Failed to parse Ollama response")?;

        let entities = Self::parse_response(&ollama_resp.response)?;
        Ok(entities)
    }

    /// Parse the JSON string returned by Ollama into a list of entities.
    /// Handles both `{"entities": [...]}` and bare `[...]` formats.
    fn parse_response(json_str: &str) -> Result<Vec<ExtractedEntity>> {
        // Try {"entities": [...]} first
        if let Ok(result) = serde_json::from_str::<ExtractionResult>(json_str) {
            return Ok(Self::filter_valid_entities(result.entities));
        }

        // Try bare array
        if let Ok(entities) = serde_json::from_str::<Vec<ExtractedEntity>>(json_str) {
            return Ok(Self::filter_valid_entities(entities));
        }

        anyhow::bail!("Failed to parse Ollama response as entity JSON: {}", json_str)
    }

    /// Filter out entities with invalid types or empty names.
    fn filter_valid_entities(entities: Vec<ExtractedEntity>) -> Vec<ExtractedEntity> {
        entities
            .into_iter()
            .filter(|e| {
                !e.name.trim().is_empty()
                    && VALID_ENTITY_TYPES.contains(&e.entity_type.as_str())
                    && e.confidence >= 0.5
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Rust"), "rust");
        assert_eq!(slugify("Machine Learning"), "machine-learning");
        assert_eq!(slugify("C++"), "c");
        assert_eq!(slugify("  Hello   World  "), "hello-world");
        assert_eq!(slugify("knowledge-nexus"), "knowledge-nexus");
    }

    #[test]
    fn test_entity_id() {
        assert_eq!(entity_id("tool", "Rust"), "tool:rust");
        assert_eq!(entity_id("topic", "Machine Learning"), "topic:machine-learning");
        assert_eq!(entity_id("person", "Linus Torvalds"), "person:linus-torvalds");
    }

    #[test]
    fn test_parse_response_wrapped() {
        let json = r#"{"entities": [
            {"name": "Rust", "entity_type": "tool", "description": "A language", "confidence": 0.95, "excerpt": "written in Rust"},
            {"name": "Tokio", "entity_type": "tool", "description": "Async runtime", "confidence": 0.9, "excerpt": "using Tokio"}
        ]}"#;
        let entities = EntityExtractor::parse_response(json).unwrap();
        assert_eq!(entities.len(), 2);
        assert_eq!(entities[0].name, "Rust");
        assert_eq!(entities[1].entity_type, "tool");
    }

    #[test]
    fn test_parse_response_bare_array() {
        let json = r#"[
            {"name": "SurrealDB", "entity_type": "tool", "description": "Multi-model DB", "confidence": 0.85}
        ]"#;
        let entities = EntityExtractor::parse_response(json).unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].name, "SurrealDB");
    }

    #[test]
    fn test_parse_response_filters_invalid_type() {
        let json = r#"{"entities": [
            {"name": "Rust", "entity_type": "tool", "confidence": 0.9},
            {"name": "Something", "entity_type": "invalid_type", "confidence": 0.8}
        ]}"#;
        let entities = EntityExtractor::parse_response(json).unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].name, "Rust");
    }

    #[test]
    fn test_parse_response_filters_low_confidence() {
        let json = r#"{"entities": [
            {"name": "Rust", "entity_type": "tool", "confidence": 0.9},
            {"name": "Maybe", "entity_type": "concept", "confidence": 0.3}
        ]}"#;
        let entities = EntityExtractor::parse_response(json).unwrap();
        assert_eq!(entities.len(), 1);
    }

    #[test]
    fn test_parse_response_filters_empty_name() {
        let json = r#"{"entities": [
            {"name": "", "entity_type": "tool", "confidence": 0.9},
            {"name": "Rust", "entity_type": "tool", "confidence": 0.9}
        ]}"#;
        let entities = EntityExtractor::parse_response(json).unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].name, "Rust");
    }

    #[test]
    fn test_parse_response_invalid_json() {
        let result = EntityExtractor::parse_response("not json at all");
        assert!(result.is_err());
    }

    #[test]
    fn test_build_prompt_contains_title_and_content() {
        let prompt = EntityExtractor::build_prompt("My Title", "My content here");
        assert!(prompt.contains("My Title"));
        assert!(prompt.contains("My content here"));
        assert!(prompt.contains("entity_type"));
    }

    #[test]
    fn test_default_confidence() {
        let json = r#"{"entities": [
            {"name": "Rust", "entity_type": "tool"}
        ]}"#;
        let entities = EntityExtractor::parse_response(json).unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].confidence, 0.5);
    }
}
```

- [ ] **Step 2: Add module declaration**

In `src/knowledge/mod.rs`, add after the existing lines:

```rust
pub mod entity_extractor;

pub use entity_extractor::EntityExtractor;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test --lib knowledge::entity_extractor::tests -- 2>&1 | tail -10`
Expected: PASS -- all unit tests pass (no Ollama needed for parsing tests).

- [ ] **Step 4: Commit**

```bash
git add src/knowledge/entity_extractor.rs src/knowledge/mod.rs
git commit -m "feat(knowledge): add EntityExtractor with Ollama HTTP client, prompt, and JSON parsing"
```

---

### Task 8: Wire dedup into ArticleService.create()

**Files:**
- Modify: `src/knowledge/articles.rs`

- [ ] **Step 1: Add dedup check to ArticleService.create()**

Modify the `create` method in `src/knowledge/articles.rs`. The dedup check goes before `self.db.create_article(article).await?`. Replace the existing `create` method with:

```rust
    /// Create an article, auto-embed it into vector store.
    ///
    /// Performs a content-hash dedup check first. If an article with the same
    /// hash already exists in the same store, the incoming article is queued
    /// for human review and the existing article's ID is returned instead.
    pub async fn create(&self, article: &Article, store_collection: &str) -> Result<CreateResult> {
        // --- P3 dedup check ---
        if !article.content_hash.is_empty() {
            if let Some(existing) = self
                .db
                .find_article_by_hash(&article.store_id, &article.content_hash)
                .await?
            {
                tracing::info!(
                    "Dedup: incoming article '{}' matches existing '{}' (hash {})",
                    article.title,
                    existing.id,
                    article.content_hash,
                );
                let entry = crate::store::DedupQueueEntry {
                    id: uuid::Uuid::new_v4().to_string(),
                    store_id: article.store_id.clone(),
                    incoming_title: article.title.clone(),
                    incoming_content: article.content.clone(),
                    incoming_source_type: article.source_type.clone(),
                    incoming_source_id: if article.source_id.is_empty() {
                        None
                    } else {
                        Some(article.source_id.clone())
                    },
                    matched_article_id: existing.id.clone(),
                    content_hash: article.content_hash.clone(),
                    status: "pending".into(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                    resolved_at: None,
                };
                self.db.create_dedup_entry(&entry).await?;
                return Ok(CreateResult::Duplicate { existing_id: existing.id });
            }
        }

        self.db.create_article(article).await?;

        // Chunk and embed
        self.embed_article(article, store_collection).await?;

        // Mark as embedded
        let mut updated = article.clone();
        updated.embedded_at = Some(chrono::Utc::now().to_rfc3339());
        updated.updated_at = chrono::Utc::now().to_rfc3339();
        self.db.update_article(&updated).await?;

        info!("Created and embedded article: {}", article.title);
        Ok(CreateResult::Created)
    }
```

- [ ] **Step 2: Add the CreateResult enum**

Add above the `impl ArticleService` block:

```rust
/// Result of an article creation attempt.
#[derive(Debug)]
pub enum CreateResult {
    /// Article was created and embedded successfully.
    Created,
    /// Article was a duplicate; queued for review. Contains the existing article's ID.
    Duplicate { existing_id: String },
}
```

- [ ] **Step 3: Update callers of ArticleService.create()**

The return type of `create()` changed from `Result<()>` to `Result<CreateResult>`. Update callers to handle both variants. Search for all call sites:

In `src/knowledge/extraction.rs`, update the call in `extract_from_conversation`:

```rust
        match self.article_service.create(&article, store_collection).await? {
            crate::knowledge::articles::CreateResult::Created => {}
            crate::knowledge::articles::CreateResult::Duplicate { existing_id } => {
                tracing::info!(
                    "Conversation extract was a duplicate of article {}",
                    existing_id
                );
            }
        }
```

Search for any other callers with `grep -r "\.create(" src/ --include="*.rs" | grep article_service` and update them similarly. Each caller should handle `CreateResult::Duplicate` by logging and continuing (not failing).

- [ ] **Step 4: Verify compilation**

Run: `cargo check 2>&1 | tail -10`
Expected: no errors

- [ ] **Step 5: Commit**

```bash
git add src/knowledge/articles.rs src/knowledge/extraction.rs
git commit -m "feat(knowledge): wire content-hash dedup check into ArticleService.create()"
```

---

### Task 9: Wire extraction into ArticleService.embed_article()

**Files:**
- Modify: `src/knowledge/articles.rs`

- [ ] **Step 1: Add extractor field to ArticleService**

Update the `ArticleService` struct and `new()` constructor:

```rust
use crate::config::ExtractionConfig;
use crate::knowledge::entity_extractor::{EntityExtractor, entity_id, slugify};

pub struct ArticleService {
    db: Arc<dyn Store>,
    vectordb: Arc<VectorDB>,
    embedding_model: Arc<Mutex<EmbeddingModel>>,
    extractor: Option<EntityExtractor>,
}

impl ArticleService {
    pub fn new(
        db: Arc<dyn Store>,
        vectordb: Arc<VectorDB>,
        embedding_model: Arc<Mutex<EmbeddingModel>>,
        extraction_config: Option<ExtractionConfig>,
    ) -> Self {
        let extractor = extraction_config
            .filter(|c| c.enabled)
            .map(EntityExtractor::new);
        Self {
            db,
            vectordb,
            embedding_model,
            extractor,
        }
    }
```

- [ ] **Step 2: Add extraction to embed_article()**

At the end of the `embed_article()` method, after `self.vectordb.insert_document(...)`, add:

```rust
        // --- P3 entity extraction ---
        if let Some(ref extractor) = self.extractor {
            match extractor.extract(&article.title, &article.content).await {
                Ok(extracted) => {
                    if let Err(e) = self.process_extracted_entities(article, &extracted).await {
                        tracing::warn!(
                            "Entity extraction post-processing failed for article '{}': {}",
                            article.id, e
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Entity extraction failed for article '{}': {} — skipping",
                        article.id, e
                    );
                }
            }
        }
```

- [ ] **Step 3: Add process_extracted_entities helper**

Add as a new method on `ArticleService`:

```rust
    /// Process entities extracted by the LLM: upsert entities, create MENTIONS
    /// edges, and compute RELATED_TO edges with other articles sharing entities.
    async fn process_extracted_entities(
        &self,
        article: &Article,
        extracted: &[crate::knowledge::entity_extractor::ExtractedEntity],
    ) -> Result<()> {
        if extracted.is_empty() {
            return Ok(());
        }

        let now = chrono::Utc::now().to_rfc3339();
        let mut entity_ids = Vec::new();

        for ext in extracted {
            let eid = entity_id(&ext.entity_type, &ext.name);

            // Upsert entity: check if it exists, increment mention_count
            let existing = self.db.get_entity(&eid).await?;
            let entity = crate::store::Entity {
                id: eid.clone(),
                name: ext.name.clone(),
                entity_type: ext.entity_type.clone(),
                description: ext.description.clone(),
                store_id: article.store_id.clone(),
                mention_count: existing.as_ref().map_or(1, |e| e.mention_count + 1),
                created_at: existing.as_ref().map_or_else(|| now.clone(), |e| e.created_at.clone()),
                updated_at: now.clone(),
            };
            self.db.upsert_entity(&entity).await?;

            // Create MENTIONS edge
            let excerpt = ext.excerpt.clone().unwrap_or_default();
            self.db
                .create_mentions_edge(&article.id, &eid, &excerpt, ext.confidence)
                .await?;

            entity_ids.push(eid);
        }

        // Compute RELATED_TO edges: find other articles sharing these entities
        self.compute_related_to_edges(article, &entity_ids).await?;

        tracing::info!(
            "Extracted {} entities for article '{}'",
            entity_ids.len(),
            article.title
        );
        Ok(())
    }

    /// For each entity this article mentions, find other articles that also
    /// mention it. For each pair, compute Jaccard similarity and create or
    /// update a RELATED_TO edge.
    async fn compute_related_to_edges(
        &self,
        article: &Article,
        entity_ids: &[String],
    ) -> Result<()> {
        let my_entity_count = entity_ids.len() as f64;
        // Collect all related article IDs and how many entities they share
        let mut shared_counts: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();

        for eid in entity_ids {
            let articles = self.db.list_articles_for_entity(eid).await?;
            for other in &articles {
                if other.id != article.id {
                    *shared_counts.entry(other.id.clone()).or_insert(0) += 1;
                }
            }
        }

        for (other_id, shared) in &shared_counts {
            // Get entity count for the other article
            let other_entities = self.db.list_entities_for_article(other_id).await?;
            let other_count = other_entities.len() as f64;
            // Jaccard: shared / (A + B - shared)
            let denominator = my_entity_count + other_count - *shared as f64;
            let strength = if denominator > 0.0 {
                *shared as f64 / denominator
            } else {
                0.0
            };
            self.db
                .create_or_update_related_to_edge(&article.id, other_id, *shared, strength)
                .await?;
        }

        Ok(())
    }
```

- [ ] **Step 4: Update all callers of ArticleService::new()**

Search for `ArticleService::new(` in the codebase and add the `extraction_config` parameter. Each call site needs to pass `Some(config.extraction.clone())` or `None`.

For example, in the K2K server initialization or wherever `ArticleService` is constructed:

```rust
// Before:
// ArticleService::new(db.clone(), vdb.clone(), emb.clone())
// After:
ArticleService::new(db.clone(), vdb.clone(), emb.clone(), Some(config.extraction.clone()))
```

For test contexts that don't have a config, pass `None`:

```rust
ArticleService::new(db.clone(), vdb.clone(), emb.clone(), None)
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check 2>&1 | tail -10`
Expected: no errors

- [ ] **Step 6: Commit**

```bash
git add src/knowledge/articles.rs
git commit -m "feat(knowledge): wire entity extraction into embed_article() with RELATED_TO computation"
```

---

### Task 10: Tag migration

**Files:**
- Modify: `src/store/migrations.rs`

- [ ] **Step 1: Add P3 migration logic**

Replace the contents of `src/store/migrations.rs`:

```rust
//! Schema version tracking and data migrations for SurrealDB.
//!
//! On every `SurrealStore::open`, we run `schema::ddl()` (which is idempotent),
//! then record the version in the `_schema_version` table.
//!
//! P3 adds a data migration: tags are converted from the article JSON array
//! into `tag` records with `TAGGED` edges.

use anyhow::{Context, Result};
use surrealdb::engine::any::Any;
use surrealdb::Surreal;

use super::schema;

/// Row shape for reading articles during tag migration.
#[derive(serde::Deserialize)]
struct ArticleTagRow {
    id: String,
    store_id: String,
    tags: serde_json::Value,
}

pub async fn run_migrations(db: &Surreal<Any>) -> Result<()> {
    // Apply DDL (idempotent — IF NOT EXISTS / OVERWRITE)
    db.query(schema::ddl())
        .await
        .context("Failed to apply SurrealDB DDL")?
        .check()
        .context("SurrealDB DDL returned an error")?;

    // Check current schema version
    let mut resp = db
        .query("SELECT version FROM _schema_version WHERE id = type::thing('_schema_version', 'current')")
        .await
        .context("Failed to read schema version")?;
    let versions: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    let current_version = versions
        .first()
        .and_then(|v| v.get("version"))
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0");

    // Run P3 data migration if upgrading from P1
    if current_version.starts_with("1.0.0-p1") || current_version.starts_with("1.0.0-p2") {
        tracing::info!("Running P3 tag migration from version {}", current_version);
        migrate_tags_to_edges(db).await?;
    }

    // Record current schema version
    let applied_at = chrono::Utc::now().to_rfc3339();
    db.query(
        "UPSERT type::thing('_schema_version', 'current') CONTENT { version: $version, applied_at: $applied_at }",
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

/// Migrate article tags from the JSON array field to `tag` records + `TAGGED` edges.
///
/// For each article with a non-empty `tags` array:
/// 1. For each tag string: upsert a `tag` record (slugified ID, scoped to store_id).
/// 2. Create a `TAGGED` edge from the article to the tag.
/// 3. After all articles are processed, remove the `tags` field from the article schema.
async fn migrate_tags_to_edges(db: &Surreal<Any>) -> Result<()> {
    // Read all articles with their tags
    let mut resp = db
        .query("SELECT meta::id(id) AS id, store_id, tags FROM article")
        .await
        .context("Failed to read articles for tag migration")?;
    let articles: Vec<ArticleTagRow> = resp.take(0).unwrap_or_default();

    let mut tag_count = 0u64;
    let mut edge_count = 0u64;
    let now = chrono::Utc::now().to_rfc3339();

    for article in &articles {
        let tags = match &article.tags {
            serde_json::Value::Array(arr) => arr.clone(),
            _ => continue,
        };

        for tag_val in &tags {
            let tag_name = match tag_val.as_str() {
                Some(s) if !s.trim().is_empty() => s.trim().to_string(),
                _ => continue,
            };

            let tag_id = slugify_tag(&tag_name);

            // Upsert tag record
            db.query(
                "UPSERT type::thing('tag', $id) CONTENT {
                    name: $name, store_id: $store_id,
                    created_at: $created_at
                }",
            )
            .bind(("id", tag_id.clone()))
            .bind(("name", tag_name))
            .bind(("store_id", article.store_id.clone()))
            .bind(("created_at", now.clone()))
            .await
            .context("Failed to upsert tag during migration")?
            .check()?;
            tag_count += 1;

            // Create TAGGED edge (ignore errors from duplicate edges)
            let edge_result = db
                .query(
                    "RELATE type::thing('article', $article_id)->tagged->type::thing('tag', $tag_id) CONTENT {
                        created_at: $created_at
                    }",
                )
                .bind(("article_id", article.id.clone()))
                .bind(("tag_id", tag_id))
                .bind(("created_at", now.clone()))
                .await;

            match edge_result {
                Ok(mut r) => { let _ = r.check(); }
                Err(e) => {
                    tracing::warn!("Skipping duplicate TAGGED edge for article {}: {}", article.id, e);
                }
            }
            edge_count += 1;
        }
    }

    // Remove the tags field from the article schema
    let remove_result = db
        .query("REMOVE FIELD IF EXISTS tags ON article; REMOVE FIELD IF EXISTS tags.* ON article;")
        .await;
    match remove_result {
        Ok(mut r) => { let _ = r.check(); }
        Err(e) => {
            tracing::warn!("Failed to remove tags field from article schema: {}", e);
        }
    }

    tracing::info!(
        "P3 tag migration complete: {} tags upserted, {} TAGGED edges created",
        tag_count, edge_count
    );
    Ok(())
}

/// Slugify a tag name for use as a tag record ID.
fn slugify_tag(s: &str) -> String {
    let mut slug = String::with_capacity(s.len());
    let mut last_was_hyphen = true;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            slug.extend(ch.to_lowercase());
            last_was_hyphen = false;
        } else if !last_was_hyphen {
            slug.push('-');
            last_was_hyphen = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}
```

- [ ] **Step 2: Handle Article.tags field removal in models.rs**

Since the `tags` field will be removed from the DB schema after migration, the `Article` struct needs `#[serde(default)]` on the `tags` field so deserialization works even when the field is absent. In `src/store/models.rs`, update:

```rust
    /// JSON array of tag strings — normalized into a `tag` table + `TAGGED`
    /// edges in P3. After P3 migration, this field is removed from the DB schema.
    /// `#[serde(default)]` ensures deserialization still works when the field is absent.
    #[serde(default)]
    pub tags: serde_json::Value,
```

- [ ] **Step 3: Update article DDL references**

In `src/store/schema.rs`, the `tags` and `tags.*` field definitions remain in the DDL because it uses `IF NOT EXISTS` -- they will be defined on first run, then `REMOVE FIELD` in the migration removes them. The DDL should **not** be modified to remove these lines yet (the migration handles removal). However, after the migration runs successfully on all databases, a future cleanup can remove them.

No changes needed to `schema.rs` for this step -- the DDL stays as-is with the `IF NOT EXISTS` guards.

- [ ] **Step 4: Verify compilation and schema tests**

Run: `cargo test --lib store -- 2>&1 | tail -10`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/store/migrations.rs src/store/models.rs
git commit -m "feat(store): add P3 tag migration from article JSON to tag records + TAGGED edges"
```

---

### Task 11: CLI -- dedup-review command

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Add DedupReview subcommand**

Add to the `Commands` enum in `src/main.rs`:

```rust
    /// Review duplicate article queue
    DedupReview {
        #[command(subcommand)]
        action: DedupReviewAction,
    },
```

Add the nested enum after `PathsAction`:

```rust
#[derive(Subcommand)]
enum DedupReviewAction {
    /// List pending duplicate entries
    List {
        /// Store ID to filter by
        #[arg(long)]
        store: String,
    },

    /// Reject a duplicate (discard incoming content)
    Reject {
        /// Dedup queue entry ID
        id: String,
    },

    /// Merge a duplicate (replace existing article with incoming content)
    Merge {
        /// Dedup queue entry ID
        id: String,
    },
}
```

- [ ] **Step 2: Add match arm in main()**

In the `match cli.command { ... }` block in the `rt.block_on` closure, add:

```rust
            Commands::DedupReview { action } => {
                cmd_dedup_review(action).await?;
            }
```

- [ ] **Step 3: Implement cmd_dedup_review()**

Add the handler function:

```rust
async fn cmd_dedup_review(action: DedupReviewAction) -> Result<()> {
    let cfg = config::load_config().await?;
    let db = open_store_or_bail(&cfg).await?;

    match action {
        DedupReviewAction::List { store } => {
            let pending = db.list_pending_dedup(&store).await?;
            if pending.is_empty() {
                println!("No pending duplicates for store '{}'.", store);
                return Ok(());
            }
            println!("Pending duplicates ({}):\n", pending.len());
            for entry in &pending {
                println!(
                    "  ID: {}\n  Incoming: \"{}\"\n  Matches: article {}\n  Hash: {}\n  Date: {}\n",
                    entry.id, entry.incoming_title, entry.matched_article_id,
                    entry.content_hash, entry.created_at,
                );
            }
        }
        DedupReviewAction::Reject { id } => {
            let entry = db
                .get_dedup_entry(&id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Dedup entry '{}' not found", id))?;
            if entry.status != "pending" {
                anyhow::bail!("Entry '{}' is already resolved (status: {})", id, entry.status);
            }
            db.resolve_dedup_entry(&id, "rejected").await?;
            println!("Rejected duplicate '{}'. Incoming content discarded.", id);
        }
        DedupReviewAction::Merge { id } => {
            let entry = db
                .get_dedup_entry(&id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Dedup entry '{}' not found", id))?;
            if entry.status != "pending" {
                anyhow::bail!("Entry '{}' is already resolved (status: {})", id, entry.status);
            }

            // Get the existing article
            let existing = db
                .get_article(&entry.matched_article_id)
                .await?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Matched article '{}' no longer exists",
                        entry.matched_article_id
                    )
                })?;

            // Update the existing article with the incoming content
            let now = chrono::Utc::now().to_rfc3339();
            let new_hash = store::hash::content_hash(&entry.incoming_content);
            let mut updated = existing;
            updated.title = entry.incoming_title.clone();
            updated.content = entry.incoming_content.clone();
            updated.content_hash = new_hash;
            updated.updated_at = now;
            updated.embedded_at = None; // will be re-embedded

            db.update_article(&updated).await?;

            // Re-embed using ArticleService (needs VectorDB + embedding model)
            let store_record = db
                .get_store(&updated.store_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Store '{}' not found", updated.store_id))?;

            let registry = vectordb::quantizer::QuantizerRegistry::new();
            let quantizer = registry.resolve(&store_record.quantizer_version)?;
            let vdb = std::sync::Arc::new(vectordb::VectorDB::open(quantizer).await?);
            let emb = embeddings::EmbeddingModel::new()?;
            let emb_arc = std::sync::Arc::new(tokio::sync::Mutex::new(emb));

            let article_svc = knowledge::ArticleService::new(
                db.clone(), vdb, emb_arc, Some(cfg.extraction.clone()),
            );
            article_svc
                .update(&updated, &store_record.lancedb_collection)
                .await?;

            db.resolve_dedup_entry(&id, "merged").await?;
            println!(
                "Merged duplicate '{}' into article '{}'. Content updated and re-embedded.",
                id, entry.matched_article_id
            );
        }
    }

    Ok(())
}
```

- [ ] **Step 4: Verify compilation**

Run: `cargo check 2>&1 | tail -10`
Expected: no errors

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat(cli): add dedup-review list/reject/merge CLI subcommands"
```

---

### Task 12: CLI -- extract-entities backfill command

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Add ExtractEntities subcommand**

Add to the `Commands` enum:

```rust
    /// Backfill entity extraction for articles missing MENTIONS edges
    ExtractEntities {
        /// Store ID to process
        #[arg(long)]
        store: String,

        /// Maximum number of articles to process (default: all)
        #[arg(long)]
        limit: Option<usize>,
    },
```

- [ ] **Step 2: Add match arm in main()**

```rust
            Commands::ExtractEntities { store, limit } => {
                cmd_extract_entities(&store, limit).await?;
            }
```

- [ ] **Step 3: Implement cmd_extract_entities()**

```rust
async fn cmd_extract_entities(store_id: &str, limit: Option<usize>) -> Result<()> {
    let cfg = config::load_config().await?;

    if !cfg.extraction.enabled {
        anyhow::bail!(
            "Entity extraction is disabled in configuration (extraction.enabled=false)"
        );
    }

    let db = open_store_or_bail(&cfg).await?;

    let store_record = db
        .get_store(store_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Store '{}' not found", store_id))?;

    // Find articles without MENTIONS edges
    let mut articles = db.list_articles_without_mentions(store_id).await?;
    if let Some(max) = limit {
        articles.truncate(max);
    }

    if articles.is_empty() {
        println!("All articles in store '{}' already have entity extractions.", store_id);
        return Ok(());
    }

    println!(
        "Backfilling entity extraction for {} articles in store '{}'...",
        articles.len(),
        store_record.name
    );

    let registry = vectordb::quantizer::QuantizerRegistry::new();
    let quantizer = registry.resolve(&store_record.quantizer_version)?;
    let vdb = std::sync::Arc::new(vectordb::VectorDB::open(quantizer).await?);
    let emb = embeddings::EmbeddingModel::new()?;
    let emb_arc = std::sync::Arc::new(tokio::sync::Mutex::new(emb));

    let article_svc = knowledge::ArticleService::new(
        db.clone(), vdb, emb_arc, Some(cfg.extraction.clone()),
    );

    let extractor = knowledge::EntityExtractor::new(cfg.extraction.clone());

    let mut success_count = 0u64;
    let mut error_count = 0u64;

    for (i, article) in articles.iter().enumerate() {
        print!("  [{}/{}] {} ... ", i + 1, articles.len(), article.title);

        match extractor.extract(&article.title, &article.content).await {
            Ok(entities) => {
                if entities.is_empty() {
                    println!("no entities found");
                } else {
                    // Use the article service's internal method indirectly by
                    // re-running embed_article which includes extraction
                    // Instead, directly process entities here
                    if let Err(e) = article_svc
                        .backfill_entities(&article, &entities)
                        .await
                    {
                        println!("ERROR: {}", e);
                        error_count += 1;
                        continue;
                    }
                    println!("{} entities", entities.len());
                    success_count += 1;
                }
            }
            Err(e) => {
                println!("ERROR: {}", e);
                error_count += 1;
            }
        }
    }

    println!(
        "\nBackfill complete: {} succeeded, {} errors, {} total",
        success_count, error_count, articles.len()
    );
    Ok(())
}
```

- [ ] **Step 4: Add backfill_entities() to ArticleService**

In `src/knowledge/articles.rs`, add a public method:

```rust
    /// Backfill entities for an already-stored article. Called by the
    /// `extract-entities` CLI command for articles that were stored before P3.
    pub async fn backfill_entities(
        &self,
        article: &Article,
        entities: &[crate::knowledge::entity_extractor::ExtractedEntity],
    ) -> Result<()> {
        self.process_extracted_entities(article, entities).await
    }
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check 2>&1 | tail -10`
Expected: no errors

- [ ] **Step 6: Commit**

```bash
git add src/main.rs src/knowledge/articles.rs
git commit -m "feat(cli): add extract-entities backfill command for articles without MENTIONS edges"
```

---

### Task 13: Integration tests

**Files:**
- Modify: `src/store/mod.rs` (add integration test modules)

- [ ] **Step 1: Write full-pipeline entity extraction test**

Add a new test module in `src/store/mod.rs`:

```rust
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

        // Create RELATED_TO edge: a1 and a2 share 2 entities out of 2+2-2=2, strength=1.0
        s.create_or_update_related_to_edge("a1", "a2", 2, 1.0).await.unwrap();

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
}
```

- [ ] **Step 2: Run all tests**

Run: `cargo test --lib 2>&1 | tail -20`
Expected: All tests PASS, including all new P3 tests.

- [ ] **Step 3: Run entity_extractor unit tests specifically**

Run: `cargo test --lib knowledge::entity_extractor::tests -- 2>&1 | tail -10`
Expected: PASS -- all parsing, slugify, entity_id tests pass.

- [ ] **Step 4: Run full test suite**

Run: `cargo test 2>&1 | tail -20`
Expected: All tests PASS. No regressions in P1/P2 tests.

- [ ] **Step 5: Commit**

```bash
git add src/store/mod.rs
git commit -m "test(store): add P3 integration tests for graph pipeline, dedup flow, tag migration"
```

---

## Post-implementation checklist

- [ ] `cargo test` passes with no failures
- [ ] `cargo clippy -- -D warnings` passes
- [ ] Schema version reads `1.0.0-p3`
- [ ] `dedup-review list --store <id>` shows pending queue
- [ ] `dedup-review reject <id>` resolves an entry
- [ ] `dedup-review merge <id>` replaces content and re-embeds
- [ ] `extract-entities --store <id>` finds articles without MENTIONS edges and processes them
- [ ] Tag migration converts existing article tags to tag records + TAGGED edges
- [ ] Entity extraction works when Ollama is running (manual test)
- [ ] Entity extraction degrades gracefully when Ollama is down (article still stored)
- [ ] `RELATED_TO` edges are created between articles sharing entities

---

### Critical Files for Implementation
- `/Users/jared.cluff/gitrepos/knowledge-nexus-local/src/store/schema.rs`
- `/Users/jared.cluff/gitrepos/knowledge-nexus-local/src/store/mod.rs`
- `/Users/jared.cluff/gitrepos/knowledge-nexus-local/src/knowledge/entity_extractor.rs`
- `/Users/jared.cluff/gitrepos/knowledge-nexus-local/src/knowledge/articles.rs`
- `/Users/jared.cluff/gitrepos/knowledge-nexus-local/src/store/migrations.rs`