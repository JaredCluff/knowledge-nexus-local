# P5: Decoupled Multi-Graph — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Prerequisite:** P4 (graph-powered tri-signal retrieval) must be merged before P5 begins. If P4 isn't merged, execute its plan first: `docs/superpowers/plans/2026-04-18-p4-graph-powered-retrieval.md`.

**Goal:** Replace the single entangled `RELATED_TO` graph with four orthogonal edge-typed subgraphs (entity-overlap, semantic, temporal, causal) plus a citation edge type, all carrying provenance metadata. Make graph traversal (P4's `GraphSearcher`) edge-type-aware. Per-graph backfill uses the cheapest method that yields the signal: temporal is SQL-only, semantic is LanceDB ANN, citations are markdown parsing, only causal requires Ollama.

**Architecture:** New edge tables in SurrealDB (`entity_overlap`, `semantically_related`, `precedes`, `caused_by`, `references_edge`) all carrying `confidence: float`, `extraction_method: string`, `created_at: string`, `store_id: string`. Migration renames `related_to` → `entity_overlap` and runs the four backfill paths in priority order. A new `RelationExtractor` orchestrator dispatches each backfill method; the existing `EntityExtractor` pattern is the template for the new `CausalExtractor`. `GraphSearcher` gets a `traverse_edge_types: Vec<EdgeType>` parameter; default config preserves P4 behavior.

**Tech Stack:** Rust, SurrealDB 2.x (RELATE, edge tables, RELATION TYPE), LanceDB (ANN scan for semantic backfill), reqwest (Ollama HTTP, mirrors `EntityExtractor`), serde_json, tokio, async-trait, chrono, tracing, clap (CLI subcommands).

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Modify | `src/store/schema.rs` | Add edge-table DDL for `entity_overlap`, `semantically_related`, `precedes`, `caused_by`, `references_edge`; bump `SCHEMA_VERSION` to `1.0.0-p5` |
| Modify | `src/store/models.rs` | Add `EntityOverlapEdge` (renamed shape), `SemanticallyRelatedEdge`, `PrecedesEdge`, `CausedByEdge`, `ReferencesEdgeRow`; add `ExtractionMethod` enum |
| Modify | `src/store/mod.rs` | Add typed-edge CRUD helpers on `Store` trait + `SurrealStore` impl; generic `create_typed_edge` plus per-type list helpers |
| Modify | `src/store/migrations.rs` | P5 migration: rename `related_to` → `entity_overlap` (data copy + drop old), run temporal backfill, set schema version |
| Create | `src/knowledge/relation_extractor.rs` | Orchestrator that dispatches to per-method backfill; idempotent; per-edge-type counts |
| Create | `src/knowledge/temporal_backfill.rs` | Deterministic `PRECEDES` from `created_at` ordering within entity-overlap clusters |
| Create | `src/knowledge/semantic_backfill.rs` | LanceDB ANN scan with cosine threshold (default 0.85); emits `semantically_related` edges |
| Create | `src/knowledge/citation_backfill.rs` | Markdown link parser; emits `references_edge` for explicit `[text](article_id)` patterns |
| Create | `src/knowledge/causal_extractor.rs` | LLM-only causal extraction via Ollama; mirrors `EntityExtractor` shape; emits `caused_by` edges |
| Modify | `src/knowledge/mod.rs` | Export the five new modules |
| Modify | `src/retrieval/graph.rs` (from P4) | Add `traverse_edge_types` param to `GraphSearcher::search`; per-type hop budget |
| Modify | `src/config/mod.rs` | Add `GraphConfig` struct with per-edge-type weights, hop budgets, extraction toggles, cos-sim threshold |
| Modify | `src/main.rs` | Add `extract-relations` subcommand with `--temporal`, `--semantic`, `--citations`, `--causal` flags; extend `graph-debug` with per-edge-type counts |

---

### Task 1: Schema additions and `ExtractionMethod` enum

**Files:**
- Modify: `src/store/schema.rs`
- Modify: `src/store/models.rs`

- [ ] **Step 1: Write failing tests for new edge models**

Add to the `tests` module in `src/store/models.rs` (after the existing `test_dedup_queue_entry_serde_round_trip`):

```rust
    #[test]
    fn test_extraction_method_serde() {
        let m = ExtractionMethod::Heuristic;
        let s = serde_json::to_string(&m).unwrap();
        assert_eq!(s, "\"heuristic\"");
        let back: ExtractionMethod = serde_json::from_str("\"llm\"").unwrap();
        assert_eq!(back, ExtractionMethod::Llm);
    }

    #[test]
    fn test_entity_overlap_edge_serde() {
        let e = EntityOverlapEdge {
            from_article_id: "a1".into(),
            to_article_id: "a2".into(),
            shared_entity_count: 3,
            strength: 0.42,
            confidence: 0.42,
            extraction_method: ExtractionMethod::Heuristic,
            created_at: "2026-05-23T00:00:00Z".into(),
            updated_at: "2026-05-23T00:00:00Z".into(),
        };
        let j = serde_json::to_string(&e).unwrap();
        let d: EntityOverlapEdge = serde_json::from_str(&j).unwrap();
        assert_eq!(d.strength, 0.42);
        assert_eq!(d.extraction_method, ExtractionMethod::Heuristic);
    }

    #[test]
    fn test_semantically_related_edge_serde() {
        let e = SemanticallyRelatedEdge {
            from_article_id: "a1".into(),
            to_article_id: "a2".into(),
            similarity: 0.93,
            confidence: 0.93,
            extraction_method: ExtractionMethod::Derived,
            created_at: "2026-05-23T00:00:00Z".into(),
        };
        let j = serde_json::to_string(&e).unwrap();
        let d: SemanticallyRelatedEdge = serde_json::from_str(&j).unwrap();
        assert_eq!(d.similarity, 0.93);
    }

    #[test]
    fn test_precedes_edge_serde() {
        let e = PrecedesEdge {
            from_article_id: "a1".into(),
            to_article_id: "a2".into(),
            confidence: 1.0,
            extraction_method: ExtractionMethod::Heuristic,
            created_at: "2026-05-23T00:00:00Z".into(),
        };
        let j = serde_json::to_string(&e).unwrap();
        let d: PrecedesEdge = serde_json::from_str(&j).unwrap();
        assert_eq!(d.confidence, 1.0);
    }

    #[test]
    fn test_caused_by_edge_serde() {
        let e = CausedByEdge {
            from_article_id: "a1".into(),
            to_article_id: "a2".into(),
            confidence: 0.78,
            rationale: Some("explicit causal language in source".into()),
            extraction_method: ExtractionMethod::Llm,
            created_at: "2026-05-23T00:00:00Z".into(),
        };
        let j = serde_json::to_string(&e).unwrap();
        let d: CausedByEdge = serde_json::from_str(&j).unwrap();
        assert_eq!(d.confidence, 0.78);
        assert_eq!(d.rationale.as_deref(), Some("explicit causal language in source"));
    }

    #[test]
    fn test_references_edge_row_serde() {
        let e = ReferencesEdgeRow {
            from_article_id: "a1".into(),
            to_article_id: "a2".into(),
            confidence: 1.0,
            extraction_method: ExtractionMethod::UserAsserted,
            anchor_text: Some("see [the deploy retro](a2)".into()),
            created_at: "2026-05-23T00:00:00Z".into(),
        };
        let j = serde_json::to_string(&e).unwrap();
        let d: ReferencesEdgeRow = serde_json::from_str(&j).unwrap();
        assert_eq!(d.anchor_text.as_deref(), Some("see [the deploy retro](a2)"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib store::models::tests::test_extraction_method_serde 2>&1 | tail -10`
Expected: FAIL — `ExtractionMethod`, `EntityOverlapEdge`, `SemanticallyRelatedEdge`, `PrecedesEdge`, `CausedByEdge`, `ReferencesEdgeRow` not defined.

- [ ] **Step 3: Add the `ExtractionMethod` enum and edge structs**

In `src/store/models.rs`, after the existing `RelatedToEdge` struct (around line 173), add:

```rust
/// How an edge was derived. Stored as a lowercase string in SurrealDB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionMethod {
    /// Deterministic / rule-based extraction (e.g. timestamps for temporal,
    /// entity-overlap for ENTITY_OVERLAP). Cheap and reproducible.
    Heuristic,
    /// LLM-driven extraction (currently only CAUSED_BY in P5).
    Llm,
    /// User explicitly asserted this edge (e.g. markdown citation).
    UserAsserted,
    /// Derived from another signal (e.g. SEMANTICALLY_RELATED via LanceDB ANN).
    Derived,
}

/// Row returned when querying an ENTITY_OVERLAP edge (renamed from `RelatedToEdge`
/// in P5). Same Jaccard-on-shared-entities semantics as P3's `RELATED_TO`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityOverlapEdge {
    pub from_article_id: String,
    pub to_article_id: String,
    pub shared_entity_count: i64,
    pub strength: f64,
    pub confidence: f64,
    pub extraction_method: ExtractionMethod,
    pub created_at: String,
    pub updated_at: String,
}

/// Row returned when querying a SEMANTICALLY_RELATED edge. Built from
/// LanceDB ANN: `cos(embedding_i, embedding_j) > θ_sim` (default 0.85).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticallyRelatedEdge {
    pub from_article_id: String,
    pub to_article_id: String,
    pub similarity: f64,
    pub confidence: f64,
    pub extraction_method: ExtractionMethod,
    pub created_at: String,
}

/// Row returned when querying a PRECEDES edge. Built deterministically from
/// `article.created_at` ordering within an entity-overlap cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrecedesEdge {
    pub from_article_id: String,
    pub to_article_id: String,
    pub confidence: f64,
    pub extraction_method: ExtractionMethod,
    pub created_at: String,
}

/// Row returned when querying a CAUSED_BY edge. LLM-extracted; `rationale` is
/// the LLM's verbatim justification for the causal claim (stored for audit).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CausedByEdge {
    pub from_article_id: String,
    pub to_article_id: String,
    pub confidence: f64,
    pub rationale: Option<String>,
    pub extraction_method: ExtractionMethod,
    pub created_at: String,
}

/// Row returned when querying a REFERENCES_EDGE. Built from explicit markdown
/// links `[anchor](target_article_id)` inside article content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferencesEdgeRow {
    pub from_article_id: String,
    pub to_article_id: String,
    pub confidence: f64,
    pub extraction_method: ExtractionMethod,
    pub anchor_text: Option<String>,
    pub created_at: String,
}
```

- [ ] **Step 4: Run model tests to verify they pass**

Run: `cargo test --lib store::models::tests 2>&1 | tail -15`
Expected: All five new tests + existing tests PASS.

- [ ] **Step 5: Bump schema version and add edge DDL**

In `src/store/schema.rs`, change line 6:

```rust
pub const SCHEMA_VERSION: &str = "1.0.0-p5";
```

Then, at the end of the DDL string (after the `related_to` block at line 188), add (note: the `references` SurrealQL keyword forces us to name the edge table `references_edge`):

```sql
-- P5 multi-graph edge tables. Each edge carries provenance metadata
-- (confidence, extraction_method, created_at, store_id).
-- ENTITY_OVERLAP supersedes RELATED_TO; the migration renames data.

DEFINE TABLE IF NOT EXISTS entity_overlap TYPE RELATION IN article OUT article SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS shared_entity_count ON entity_overlap TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS strength ON entity_overlap TYPE float DEFAULT 0.0;
DEFINE FIELD IF NOT EXISTS confidence ON entity_overlap TYPE float DEFAULT 0.0;
DEFINE FIELD IF NOT EXISTS extraction_method ON entity_overlap TYPE string DEFAULT "heuristic";
DEFINE FIELD IF NOT EXISTS store_id ON entity_overlap TYPE string;
DEFINE FIELD IF NOT EXISTS created_at ON entity_overlap TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at ON entity_overlap TYPE string;
DEFINE INDEX IF NOT EXISTS entity_overlap_unique ON entity_overlap FIELDS in, out UNIQUE;
DEFINE INDEX IF NOT EXISTS entity_overlap_store_idx ON entity_overlap FIELDS store_id;

-- SEMANTICALLY_RELATED edge table: article -> article (cos-sim > threshold)
DEFINE TABLE IF NOT EXISTS semantically_related TYPE RELATION IN article OUT article SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS similarity ON semantically_related TYPE float;
DEFINE FIELD IF NOT EXISTS confidence ON semantically_related TYPE float;
DEFINE FIELD IF NOT EXISTS extraction_method ON semantically_related TYPE string DEFAULT "derived";
DEFINE FIELD IF NOT EXISTS store_id ON semantically_related TYPE string;
DEFINE FIELD IF NOT EXISTS created_at ON semantically_related TYPE string;
DEFINE INDEX IF NOT EXISTS semantically_related_unique
    ON semantically_related FIELDS in, out UNIQUE;
DEFINE INDEX IF NOT EXISTS semantically_related_store_idx
    ON semantically_related FIELDS store_id;

-- PRECEDES edge table: article -> article (temporal ordering)
DEFINE TABLE IF NOT EXISTS precedes TYPE RELATION IN article OUT article SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS confidence ON precedes TYPE float DEFAULT 1.0;
DEFINE FIELD IF NOT EXISTS extraction_method ON precedes TYPE string DEFAULT "heuristic";
DEFINE FIELD IF NOT EXISTS store_id ON precedes TYPE string;
DEFINE FIELD IF NOT EXISTS created_at ON precedes TYPE string;
DEFINE INDEX IF NOT EXISTS precedes_unique ON precedes FIELDS in, out UNIQUE;
DEFINE INDEX IF NOT EXISTS precedes_store_idx ON precedes FIELDS store_id;

-- CAUSED_BY edge table: article -> article (LLM-extracted causal claim)
DEFINE TABLE IF NOT EXISTS caused_by TYPE RELATION IN article OUT article SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS confidence ON caused_by TYPE float;
DEFINE FIELD IF NOT EXISTS rationale ON caused_by TYPE option<string>;
DEFINE FIELD IF NOT EXISTS extraction_method ON caused_by TYPE string DEFAULT "llm";
DEFINE FIELD IF NOT EXISTS store_id ON caused_by TYPE string;
DEFINE FIELD IF NOT EXISTS created_at ON caused_by TYPE string;
DEFINE INDEX IF NOT EXISTS caused_by_unique ON caused_by FIELDS in, out UNIQUE;
DEFINE INDEX IF NOT EXISTS caused_by_store_idx ON caused_by FIELDS store_id;

-- REFERENCES_EDGE edge table: article -> article (explicit markdown citation)
-- Named `references_edge` because `references` is a reserved SurrealQL keyword.
DEFINE TABLE IF NOT EXISTS references_edge TYPE RELATION IN article OUT article SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS confidence ON references_edge TYPE float DEFAULT 1.0;
DEFINE FIELD IF NOT EXISTS anchor_text ON references_edge TYPE option<string>;
DEFINE FIELD IF NOT EXISTS extraction_method ON references_edge TYPE string DEFAULT "user_asserted";
DEFINE FIELD IF NOT EXISTS store_id ON references_edge TYPE string;
DEFINE FIELD IF NOT EXISTS created_at ON references_edge TYPE string;
DEFINE INDEX IF NOT EXISTS references_edge_unique ON references_edge FIELDS in, out UNIQUE;
DEFINE INDEX IF NOT EXISTS references_edge_store_idx ON references_edge FIELDS store_id;
```

- [ ] **Step 6: Verify DDL compiles and applies on fresh DB**

Run: `cargo build 2>&1 | tail -5`
Expected: builds successfully.

Then a smoke test that exercises the DDL via the existing `SurrealStore::open`. If the store has integration tests, run them:

Run: `cargo test --lib store::tests 2>&1 | tail -20`
Expected: all existing store tests PASS (DDL is additive and idempotent — must not regress).

- [ ] **Step 7: Commit**

```bash
git add src/store/schema.rs src/store/models.rs
git commit -m "feat(p5): add multi-graph edge tables and ExtractionMethod enum

Adds entity_overlap, semantically_related, precedes, caused_by, and
references_edge tables to the DDL, each carrying confidence,
extraction_method, and store_id provenance metadata. Adds typed Rust
structs for each edge plus an ExtractionMethod enum. Schema version
bumped to 1.0.0-p5."
```

---

### Task 2: P5 data migration — rename `related_to` → `entity_overlap`

**Files:**
- Modify: `src/store/migrations.rs`

- [ ] **Step 1: Write a failing migration test**

Create a new test in `src/store/migrations.rs` (under a new `#[cfg(test)] mod tests` block at the bottom — there is no existing test module in this file):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::schema;
    use surrealdb::engine::any::connect;

    /// Helper: connect to an in-memory SurrealDB and seed some `related_to` edges
    /// that simulate a P3-state corpus.
    async fn setup_p3_corpus() -> Surreal<Any> {
        let db = connect("memory").await.expect("connect mem");
        db.use_ns("test").use_db("test").await.expect("use ns/db");
        db.query(schema::ddl()).await.expect("ddl").check().expect("ddl check");

        // Pretend schema version is P3
        db.query(
            "UPSERT type::thing('_schema_version', 'current') CONTENT { version: $v, applied_at: $t }"
        )
        .bind(("v", "1.0.0-p3"))
        .bind(("t", "2026-04-17T00:00:00Z"))
        .await.expect("seed p3 version").check().expect("seed p3 check");

        // Seed two articles and a related_to edge between them
        db.query(r#"
            CREATE article:a1 CONTENT { store_id: "s1", title: "A1", content: "x",
                source_type: "user", source_id: "", content_hash: "h1", tags: [],
                created_at: "2026-04-17T00:00:00Z", updated_at: "2026-04-17T00:00:00Z" };
            CREATE article:a2 CONTENT { store_id: "s1", title: "A2", content: "y",
                source_type: "user", source_id: "", content_hash: "h2", tags: [],
                created_at: "2026-04-17T01:00:00Z", updated_at: "2026-04-17T01:00:00Z" };
            RELATE article:a1->related_to->article:a2 CONTENT {
                shared_entity_count: 2, strength: 0.5,
                created_at: "2026-04-17T02:00:00Z", updated_at: "2026-04-17T02:00:00Z"
            };
        "#).await.expect("seed").check().expect("seed check");

        db
    }

    #[tokio::test]
    async fn migration_p3_to_p5_renames_related_to_to_entity_overlap() {
        let db = setup_p3_corpus().await;

        // Run migrations: should migrate P3 -> P5
        run_migrations(&db).await.expect("run migrations");

        // After migration: an entity_overlap edge exists with the same payload
        let mut resp = db.query(
            "SELECT meta::id(in) AS from_id, meta::id(out) AS to_id,
                    shared_entity_count, strength, confidence, extraction_method
             FROM entity_overlap"
        ).await.expect("query entity_overlap").check().expect("check");
        #[derive(serde::Deserialize)]
        struct Row { from_id: String, to_id: String, shared_entity_count: i64,
                     strength: f64, confidence: f64, extraction_method: String }
        let rows: Vec<Row> = resp.take(0).unwrap_or_default();

        assert_eq!(rows.len(), 1, "expected exactly one entity_overlap edge");
        assert_eq!(rows[0].from_id, "a1");
        assert_eq!(rows[0].to_id, "a2");
        assert_eq!(rows[0].shared_entity_count, 2);
        assert!((rows[0].strength - 0.5).abs() < 1e-9);
        assert!((rows[0].confidence - 0.5).abs() < 1e-9, "confidence should default to strength");
        assert_eq!(rows[0].extraction_method, "heuristic");

        // And the old related_to table is empty
        let mut resp2 = db.query("SELECT count() AS n FROM related_to GROUP ALL")
            .await.expect("count related_to").check().expect("check2");
        #[derive(serde::Deserialize)] struct Cnt { n: i64 }
        let cnts: Vec<Cnt> = resp2.take(0).unwrap_or_default();
        let n = cnts.first().map(|c| c.n).unwrap_or(0);
        assert_eq!(n, 0, "related_to should be empty after migration");

        // Schema version is now 1.0.0-p5
        let mut resp3 = db.query(
            "SELECT version FROM _schema_version WHERE id = type::thing('_schema_version', 'current')"
        ).await.expect("version").check().expect("check3");
        #[derive(serde::Deserialize)] struct V { version: String }
        let vs: Vec<V> = resp3.take(0).unwrap_or_default();
        assert_eq!(vs.first().map(|v| v.version.as_str()), Some("1.0.0-p5"));
    }

    #[tokio::test]
    async fn migration_p5_to_p5_is_noop() {
        let db = setup_p3_corpus().await;
        run_migrations(&db).await.expect("first run");
        // Second run should be a no-op (entity_overlap stays put, no errors)
        run_migrations(&db).await.expect("second run idempotent");

        let mut resp = db.query("SELECT count() AS n FROM entity_overlap GROUP ALL")
            .await.expect("count").check().expect("check");
        #[derive(serde::Deserialize)] struct Cnt { n: i64 }
        let cnts: Vec<Cnt> = resp.take(0).unwrap_or_default();
        assert_eq!(cnts.first().map(|c| c.n).unwrap_or(0), 1);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib store::migrations::tests::migration_p3_to_p5 2>&1 | tail -10`
Expected: FAIL — `entity_overlap` table empty because no migration runs the rename.

- [ ] **Step 3: Implement the P5 migration step**

In `src/store/migrations.rs`, add the migration call inside `run_migrations` (after the P3 check, before the schema-version write — around line 48 in the existing file):

```rust
    // Run P5 multi-graph migration if upgrading from P3 (or earlier)
    if current_version.starts_with("1.0.0-p1")
        || current_version.starts_with("1.0.0-p2")
        || current_version.starts_with("1.0.0-p3")
    {
        tracing::info!("Running P5 multi-graph migration from version {}", current_version);
        migrate_related_to_to_entity_overlap(db).await?;
    }
```

Then add the new migration function at the bottom of the file (before `#[cfg(test)]`):

```rust
/// P5 migration: copy `related_to` edges into the new `entity_overlap` table,
/// preserving Jaccard-derived `shared_entity_count` and `strength`, defaulting
/// `confidence = strength`, `extraction_method = "heuristic"`. Then deletes
/// all rows from `related_to`. The old table itself is kept in DDL for
/// backward compatibility but holds no data going forward.
async fn migrate_related_to_to_entity_overlap(db: &Surreal<Any>) -> Result<()> {
    // Read all related_to edges
    let mut resp = db
        .query(
            "SELECT meta::id(in) AS from_id, meta::id(out) AS to_id,
                    store_id, shared_entity_count, strength,
                    created_at, updated_at
             FROM related_to"
        )
        .await
        .context("Failed to read related_to edges during P5 migration")?;

    #[derive(serde::Deserialize)]
    struct OldEdge {
        from_id: String,
        to_id: String,
        store_id: Option<String>,
        shared_entity_count: i64,
        strength: f64,
        created_at: String,
        updated_at: String,
    }

    let edges: Vec<OldEdge> = resp.take(0).unwrap_or_default();
    let total = edges.len();
    let mut migrated = 0u64;

    for e in edges {
        let store_id = e.store_id.unwrap_or_default();
        let res = db
            .query(
                "LET $from = type::thing('article', $from_id);
                 LET $to = type::thing('article', $to_id);
                 RELATE $from->entity_overlap->$to CONTENT {
                    shared_entity_count: $cnt,
                    strength: $strength,
                    confidence: $confidence,
                    extraction_method: 'heuristic',
                    store_id: $store_id,
                    created_at: $created_at,
                    updated_at: $updated_at
                 }"
            )
            .bind(("from_id", e.from_id.clone()))
            .bind(("to_id", e.to_id.clone()))
            .bind(("cnt", e.shared_entity_count))
            .bind(("strength", e.strength))
            .bind(("confidence", e.strength))
            .bind(("store_id", store_id))
            .bind(("created_at", e.created_at))
            .bind(("updated_at", e.updated_at))
            .await;

        match res {
            Ok(mut r) => { let _ = r.check(); migrated += 1; }
            Err(err) => {
                tracing::warn!(
                    "Skipping duplicate entity_overlap edge {} -> {} during P5 migration: {}",
                    e.from_id, e.to_id, err
                );
            }
        }
    }

    // Delete all rows from the old related_to table
    db.query("DELETE related_to")
        .await
        .context("Failed to drop related_to rows after P5 migration")?
        .check()
        .context("DELETE related_to returned an error")?;

    tracing::info!(
        "P5 migration complete: {}/{} related_to edges renamed to entity_overlap",
        migrated, total
    );
    Ok(())
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib store::migrations::tests 2>&1 | tail -10`
Expected: Both `migration_p3_to_p5_renames_related_to_to_entity_overlap` and `migration_p5_to_p5_is_noop` PASS.

- [ ] **Step 5: Commit**

```bash
git add src/store/migrations.rs
git commit -m "feat(p5): migrate related_to edges to entity_overlap

Copies all P3 related_to edges into the new entity_overlap table with
extraction_method='heuristic' and confidence defaulted to strength, then
clears the old table. Migration runs once from any version starting with
1.0.0-p1/p2/p3 and is idempotent on repeat runs."
```

---

### Task 3: Typed-edge CRUD helpers on `Store`

**Files:**
- Modify: `src/store/mod.rs`

- [ ] **Step 1: Write failing tests for typed edge creation**

Add these tests inside the existing `#[cfg(test)] mod tests` block of `src/store/mod.rs`. If you don't see a test module in `store/mod.rs`, add it at the end of the file. (Reuse any existing `setup_store_with_article` helper if present; otherwise inline the setup.)

```rust
    #[tokio::test]
    async fn create_precedes_edge_round_trips() {
        let store = SurrealStore::open_memory().await.expect("open mem store");
        seed_two_articles(&store, "s1", "a1", "a2").await;

        store.create_precedes_edge(
            "s1", "a1", "a2",
            1.0, ExtractionMethod::Heuristic,
        ).await.expect("create precedes");

        let edges = store.list_precedes_for("s1", "a1").await.expect("list");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to_article_id, "a2");
        assert_eq!(edges[0].extraction_method, ExtractionMethod::Heuristic);
    }

    #[tokio::test]
    async fn create_semantically_related_edge_dedups_on_unique() {
        let store = SurrealStore::open_memory().await.expect("open mem store");
        seed_two_articles(&store, "s1", "a1", "a2").await;

        store.create_semantically_related_edge("s1", "a1", "a2", 0.91).await.expect("first");
        // Second insert of the same pair should be a no-op (UNIQUE index)
        let res = store.create_semantically_related_edge("s1", "a1", "a2", 0.95).await;
        assert!(res.is_ok(), "duplicate insert should not error; got {:?}", res);

        let edges = store.list_semantically_related_for("s1", "a1").await.expect("list");
        assert_eq!(edges.len(), 1);
    }

    #[tokio::test]
    async fn create_caused_by_edge_round_trips() {
        let store = SurrealStore::open_memory().await.expect("open mem store");
        seed_two_articles(&store, "s1", "a1", "a2").await;

        store.create_caused_by_edge(
            "s1", "a1", "a2",
            0.82, Some("explicit 'because' clause".into()),
        ).await.expect("create caused_by");

        let edges = store.list_caused_by_for("s1", "a1").await.expect("list");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].rationale.as_deref(), Some("explicit 'because' clause"));
    }

    #[tokio::test]
    async fn create_references_edge_round_trips() {
        let store = SurrealStore::open_memory().await.expect("open mem store");
        seed_two_articles(&store, "s1", "a1", "a2").await;

        store.create_references_edge(
            "s1", "a1", "a2",
            Some("see [related](a2)".into()),
        ).await.expect("create references");

        let edges = store.list_references_for("s1", "a1").await.expect("list");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].anchor_text.as_deref(), Some("see [related](a2)"));
    }

    /// Helper used by the tests above. Inserts two articles with deterministic
    /// timestamps so PRECEDES tests have meaningful ordering.
    async fn seed_two_articles(store: &SurrealStore, store_id: &str, a_id: &str, b_id: &str) {
        store.db()
            .query(format!(
                "CREATE article:{a} CONTENT {{ store_id: $sid, title: 'A', content: 'x',
                    source_type: 'user', source_id: '', content_hash: '{a}',
                    tags: [], created_at: '2026-05-23T00:00:00Z', updated_at: '2026-05-23T00:00:00Z' }};
                 CREATE article:{b} CONTENT {{ store_id: $sid, title: 'B', content: 'y',
                    source_type: 'user', source_id: '', content_hash: '{b}',
                    tags: [], created_at: '2026-05-23T01:00:00Z', updated_at: '2026-05-23T01:00:00Z' }};",
                a = a_id, b = b_id
            ))
            .bind(("sid", store_id.to_string()))
            .await.expect("seed articles").check().expect("seed check");
    }
```

> If `SurrealStore::open_memory` and a `db()` accessor don't yet exist, add them as thin helpers in `store/mod.rs` (`open_memory` calls `surrealdb::engine::any::connect("memory")` and `use_ns/use_db`; `db()` returns `&Surreal<Any>`).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib store::tests::create_precedes_edge 2>&1 | tail -10`
Expected: FAIL — methods `create_precedes_edge`, `create_semantically_related_edge`, `create_caused_by_edge`, `create_references_edge` not defined.

- [ ] **Step 3: Implement the typed-edge CRUD methods**

In `src/store/mod.rs`, in the `Store` trait, add the method signatures:

```rust
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
```

Then in the `impl Store for SurrealStore` block, add (use the existing `RELATE` and `SELECT` patterns from P3's MENTIONS/RELATED_TO implementations as templates):

```rust
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

        let res = self.db
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
        // Swallow unique-violation errors so callers can safely re-create
        match res { Ok(mut r) => { let _ = r.check(); Ok(()) } Err(_) => Ok(()) }
    }

    async fn create_semantically_related_edge(
        &self,
        store_id: &str,
        from_article_id: &str,
        to_article_id: &str,
        similarity: f64,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let res = self.db
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
        match res { Ok(mut r) => { let _ = r.check(); Ok(()) } Err(_) => Ok(()) }
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
        let res = self.db
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
        match res { Ok(mut r) => { let _ = r.check(); Ok(()) } Err(_) => Ok(()) }
    }

    async fn create_references_edge(
        &self,
        store_id: &str,
        from_article_id: &str,
        to_article_id: &str,
        anchor_text: Option<String>,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let res = self.db
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
        match res { Ok(mut r) => { let _ = r.check(); Ok(()) } Err(_) => Ok(()) }
    }

    async fn list_precedes_for(&self, store_id: &str, article_id: &str) -> Result<Vec<PrecedesEdge>> {
        let mut resp = self.db
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
        let mut resp = self.db
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
        let mut resp = self.db
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
        let mut resp = self.db
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
```

Make sure to import the new model types at the top of `src/store/mod.rs`:

```rust
use super::models::{
    /* existing imports */
    ExtractionMethod, PrecedesEdge, SemanticallyRelatedEdge, CausedByEdge, ReferencesEdgeRow,
};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib store::tests 2>&1 | tail -20`
Expected: All four new typed-edge tests PASS; existing store tests unaffected.

- [ ] **Step 5: Commit**

```bash
git add src/store/mod.rs
git commit -m "feat(p5): typed-edge CRUD on Store trait

Adds create_/list_ helpers for precedes, semantically_related, caused_by,
and references_edge tables. Duplicate inserts are silently no-op'd via the
UNIQUE indexes defined in P5 schema. Existing P3 edge methods (mentions,
tagged, related_to) are unchanged."
```

---

### Task 4: Temporal backfill (deterministic, free, no LLM)

**Files:**
- Create: `src/knowledge/temporal_backfill.rs`
- Modify: `src/knowledge/mod.rs`

- [ ] **Step 1: Write failing test for temporal backfill**

Create `src/knowledge/temporal_backfill.rs`. Add a test module at the bottom (use `#[cfg(test)]` with the same pattern as `entity_extractor.rs`):

```rust
//! Deterministic temporal-edge backfill.
//!
//! For each ENTITY_OVERLAP cluster, emit PRECEDES edges in `created_at` order.
//! No LLM cost; runs on the existing P3 entity-overlap graph.

use anyhow::Result;
use chrono::DateTime;

use crate::store::Store;

/// Per-store temporal backfill. Returns the number of PRECEDES edges created.
pub async fn backfill_temporal<S: Store + Sync>(store: &S, store_id: &str) -> Result<u64> {
    let pairs = store.list_entity_overlap_pairs(store_id).await?;
    let mut count = 0u64;

    for (a_id, b_id) in pairs {
        let a = store.get_article(store_id, &a_id).await?;
        let b = store.get_article(store_id, &b_id).await?;
        let (Some(a), Some(b)) = (a, b) else { continue };

        let a_t = parse_ts(&a.created_at);
        let b_t = parse_ts(&b.created_at);
        let (Some(a_t), Some(b_t)) = (a_t, b_t) else { continue };

        let (from, to) = if a_t < b_t { (a_id, b_id) } else if b_t < a_t { (b_id, a_id) } else { continue };
        store.create_precedes_edge(
            store_id, &from, &to, 1.0,
            crate::store::models::ExtractionMethod::Heuristic,
        ).await?;
        count += 1;
    }

    tracing::info!("Temporal backfill complete for store {}: {} PRECEDES edges", store_id, count);
    Ok(count)
}

fn parse_ts(s: &str) -> Option<DateTime<chrono::Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&chrono::Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SurrealStore;
    use crate::store::models::ExtractionMethod;

    /// Seed two articles + one entity_overlap edge; expect one PRECEDES
    /// from the earlier to the later created_at.
    #[tokio::test]
    async fn temporal_backfill_emits_one_precedes_per_overlap_pair() {
        let store = SurrealStore::open_memory().await.expect("open mem");
        // Two articles with deterministic timestamps
        store.db().query(r#"
            CREATE article:earlier CONTENT { store_id: "s1", title: "E", content: "x",
                source_type: "user", source_id: "", content_hash: "e", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:later CONTENT { store_id: "s1", title: "L", content: "y",
                source_type: "user", source_id: "", content_hash: "l", tags: [],
                created_at: "2026-02-01T00:00:00Z", updated_at: "2026-02-01T00:00:00Z" };
            RELATE article:earlier->entity_overlap->article:later CONTENT {
                shared_entity_count: 2, strength: 0.5, confidence: 0.5,
                extraction_method: "heuristic", store_id: "s1",
                created_at: "2026-02-01T00:00:01Z", updated_at: "2026-02-01T00:00:01Z"
            };
        "#).await.expect("seed").check().expect("seed check");

        let n = backfill_temporal(&store, "s1").await.expect("backfill");
        assert_eq!(n, 1);

        let edges = store.list_precedes_for("s1", "earlier").await.expect("list");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to_article_id, "later");
        assert_eq!(edges[0].extraction_method, ExtractionMethod::Heuristic);
    }

    #[tokio::test]
    async fn temporal_backfill_is_idempotent() {
        let store = SurrealStore::open_memory().await.expect("open mem");
        store.db().query(r#"
            CREATE article:a CONTENT { store_id: "s1", title: "A", content: "",
                source_type: "user", source_id: "", content_hash: "a", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:b CONTENT { store_id: "s1", title: "B", content: "",
                source_type: "user", source_id: "", content_hash: "b", tags: [],
                created_at: "2026-02-01T00:00:00Z", updated_at: "2026-02-01T00:00:00Z" };
            RELATE article:a->entity_overlap->article:b CONTENT {
                shared_entity_count: 1, strength: 0.3, confidence: 0.3,
                extraction_method: "heuristic", store_id: "s1",
                created_at: "2026-02-01T00:00:01Z", updated_at: "2026-02-01T00:00:01Z"
            };
        "#).await.expect("seed").check().expect("seed check");

        backfill_temporal(&store, "s1").await.expect("first");
        backfill_temporal(&store, "s1").await.expect("second");

        let edges = store.list_precedes_for("s1", "a").await.expect("list");
        assert_eq!(edges.len(), 1, "duplicate not added");
    }

    #[tokio::test]
    async fn temporal_backfill_skips_equal_timestamps() {
        let store = SurrealStore::open_memory().await.expect("open mem");
        store.db().query(r#"
            CREATE article:a CONTENT { store_id: "s1", title: "A", content: "",
                source_type: "user", source_id: "", content_hash: "a", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:b CONTENT { store_id: "s1", title: "B", content: "",
                source_type: "user", source_id: "", content_hash: "b", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            RELATE article:a->entity_overlap->article:b CONTENT {
                shared_entity_count: 1, strength: 0.3, confidence: 0.3,
                extraction_method: "heuristic", store_id: "s1",
                created_at: "2026-01-01T00:00:01Z", updated_at: "2026-01-01T00:00:01Z"
            };
        "#).await.expect("seed").check().expect("seed check");

        let n = backfill_temporal(&store, "s1").await.expect("backfill");
        assert_eq!(n, 0, "equal timestamps yield no PRECEDES");
    }
}
```

Also add to `src/knowledge/mod.rs`:

```rust
pub mod temporal_backfill;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib knowledge::temporal_backfill 2>&1 | tail -10`
Expected: FAIL — `Store::list_entity_overlap_pairs` and `Store::get_article` may not exist with these signatures.

- [ ] **Step 3: Add `list_entity_overlap_pairs` to the Store trait + impl**

In `src/store/mod.rs`, add to the `Store` trait:

```rust
    /// Returns all (from_id, to_id) pairs from the entity_overlap table for
    /// a given store. Used by P5 backfills that operate over the existing
    /// entity-overlap graph.
    async fn list_entity_overlap_pairs(&self, store_id: &str) -> Result<Vec<(String, String)>>;
```

And in the `SurrealStore` impl:

```rust
    async fn list_entity_overlap_pairs(&self, store_id: &str) -> Result<Vec<(String, String)>> {
        let mut resp = self.db
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
```

(If `get_article` doesn't exist on `Store`, add it now — it's a thin SELECT by id wrapper. Most P3 code already uses one.)

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib knowledge::temporal_backfill 2>&1 | tail -10`
Expected: All three temporal backfill tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/store/mod.rs src/knowledge/temporal_backfill.rs src/knowledge/mod.rs
git commit -m "feat(p5): deterministic temporal backfill

Adds a temporal_backfill module that walks the entity_overlap graph and
emits one PRECEDES edge per cluster pair in created_at order. No LLM
cost; idempotent via unique-index dedup; skips equal-timestamp pairs.
Adds list_entity_overlap_pairs helper to the Store trait."
```

---

### Task 5: Semantic backfill (LanceDB ANN, cheap, no LLM)

**Files:**
- Create: `src/knowledge/semantic_backfill.rs`
- Modify: `src/knowledge/mod.rs`

- [ ] **Step 1: Write failing test for semantic backfill**

Create `src/knowledge/semantic_backfill.rs`:

```rust
//! Semantic-edge backfill via LanceDB ANN.
//!
//! For each article with a stored embedding, query LanceDB for nearest
//! neighbors; emit a SEMANTICALLY_RELATED edge to each neighbor whose
//! cosine similarity exceeds a configurable threshold (default 0.85).

use anyhow::Result;

use crate::store::Store;
use crate::vectordb::VectorDb;

/// Per-store semantic backfill. Returns the number of edges created.
pub async fn backfill_semantic<S: Store + Sync, V: VectorDb + Sync>(
    store: &S,
    vector_db: &V,
    store_id: &str,
    threshold: f64,
    top_k: usize,
) -> Result<u64> {
    let article_ids = store.list_article_ids(store_id).await?;
    let mut count = 0u64;

    for article_id in &article_ids {
        let Some(emb) = vector_db.get_embedding(store_id, article_id).await? else { continue };
        let neighbors = vector_db.ann_query(store_id, &emb, top_k).await?;

        for (neighbor_id, similarity) in neighbors {
            if neighbor_id == *article_id { continue; }
            if similarity < threshold { continue; }
            // Lexicographic ordering ensures we don't create both A->B and B->A
            let (from, to) = if article_id.as_str() < neighbor_id.as_str() {
                (article_id.clone(), neighbor_id.clone())
            } else {
                (neighbor_id.clone(), article_id.clone())
            };
            store.create_semantically_related_edge(store_id, &from, &to, similarity).await?;
            count += 1;
        }
    }

    tracing::info!("Semantic backfill complete for store {}: {} edges (threshold={}, top_k={})",
        store_id, count, threshold, top_k);
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SurrealStore;
    use crate::vectordb::mock::MockVectorDb;

    #[tokio::test]
    async fn semantic_backfill_emits_edges_above_threshold() {
        let store = SurrealStore::open_memory().await.expect("open mem");
        store.db().query(r#"
            CREATE article:a CONTENT { store_id: "s1", title: "A", content: "",
                source_type: "user", source_id: "", content_hash: "a", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:b CONTENT { store_id: "s1", title: "B", content: "",
                source_type: "user", source_id: "", content_hash: "b", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:c CONTENT { store_id: "s1", title: "C", content: "",
                source_type: "user", source_id: "", content_hash: "c", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
        "#).await.expect("seed").check().expect("seed check");

        // a-b are similar (0.92), a-c are not (0.6)
        let mock = MockVectorDb::with_pairs("s1", &[
            ("a", &[("b", 0.92), ("c", 0.6)]),
            ("b", &[("a", 0.92), ("c", 0.6)]),
            ("c", &[("a", 0.6), ("b", 0.6)]),
        ]);

        let n = backfill_semantic(&store, &mock, "s1", 0.85, 10).await.expect("backfill");
        assert_eq!(n, 2, "one edge a->b emitted twice (once per direction, dedup'd to 1 via unique index)");

        let edges = store.list_semantically_related_for("s1", "a").await.expect("list a");
        assert_eq!(edges.len(), 1);
        assert!((edges[0].similarity - 0.92).abs() < 1e-9);
    }

    #[tokio::test]
    async fn semantic_backfill_respects_threshold() {
        let store = SurrealStore::open_memory().await.expect("open mem");
        store.db().query(r#"
            CREATE article:a CONTENT { store_id: "s1", title: "A", content: "",
                source_type: "user", source_id: "", content_hash: "a", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:b CONTENT { store_id: "s1", title: "B", content: "",
                source_type: "user", source_id: "", content_hash: "b", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
        "#).await.expect("seed").check().expect("seed check");

        // Below threshold
        let mock = MockVectorDb::with_pairs("s1", &[
            ("a", &[("b", 0.7)]),
            ("b", &[("a", 0.7)]),
        ]);
        let n = backfill_semantic(&store, &mock, "s1", 0.85, 10).await.expect("backfill");
        assert_eq!(n, 0, "below-threshold neighbors must be skipped");
    }
}
```

Also add to `src/knowledge/mod.rs`:

```rust
pub mod semantic_backfill;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib knowledge::semantic_backfill 2>&1 | tail -10`
Expected: FAIL — `Store::list_article_ids` and `VectorDb::get_embedding` / `ann_query` may need exposure.

- [ ] **Step 3: Add required Store and VectorDb helpers**

In `src/store/mod.rs`:

```rust
    async fn list_article_ids(&self, store_id: &str) -> Result<Vec<String>>;
```

In the `SurrealStore` impl:

```rust
    async fn list_article_ids(&self, store_id: &str) -> Result<Vec<String>> {
        let mut resp = self.db
            .query("SELECT meta::id(id) AS id FROM article WHERE store_id = $sid")
            .bind(("sid", store_id.to_string()))
            .await?;
        #[derive(serde::Deserialize)] struct Row { id: String }
        let rows: Vec<Row> = resp.take(0).unwrap_or_default();
        Ok(rows.into_iter().map(|r| r.id).collect())
    }
```

In `src/vectordb/mod.rs` (or wherever `VectorDb` trait lives), ensure these signatures exist:

```rust
    async fn get_embedding(&self, store_id: &str, article_id: &str) -> Result<Option<Vec<f32>>>;
    async fn ann_query(&self, store_id: &str, query_embedding: &[f32], top_k: usize) -> Result<Vec<(String, f64)>>;
```

If they don't, add thin wrappers around the existing LanceDB calls (search by row id for `get_embedding`, k-NN for `ann_query`).

Add a `MockVectorDb` in `src/vectordb/mock.rs` for test isolation:

```rust
//! Test-only mock VectorDb. Returns pre-seeded neighbor pairs.

use anyhow::Result;
use std::collections::HashMap;

use crate::vectordb::VectorDb;

pub struct MockVectorDb {
    /// Map<store_id, Map<article_id, Vec<(neighbor_id, similarity)>>>
    pairs: HashMap<String, HashMap<String, Vec<(String, f64)>>>,
}

impl MockVectorDb {
    pub fn with_pairs(store_id: &str, data: &[(&str, &[(&str, f64)])]) -> Self {
        let mut store = HashMap::new();
        for (article_id, neighbors) in data {
            store.insert(
                article_id.to_string(),
                neighbors.iter().map(|(n, s)| (n.to_string(), *s)).collect(),
            );
        }
        let mut pairs = HashMap::new();
        pairs.insert(store_id.to_string(), store);
        Self { pairs }
    }
}

#[async_trait::async_trait]
impl VectorDb for MockVectorDb {
    async fn get_embedding(&self, store_id: &str, article_id: &str) -> Result<Option<Vec<f32>>> {
        if self.pairs.get(store_id).and_then(|m| m.get(article_id)).is_some() {
            Ok(Some(vec![0.0; 384]))
        } else {
            Ok(None)
        }
    }

    async fn ann_query(&self, store_id: &str, _q: &[f32], _top_k: usize) -> Result<Vec<(String, f64)>> {
        // Mock returns pre-seeded neighbors; ignore the actual query vector.
        // We pick the first entry that has pairs (the test inserts one query at a time).
        let store = self.pairs.get(store_id).cloned().unwrap_or_default();
        // Return all pairs from the first article (test seeds expects deterministic order)
        Ok(store.values().next().cloned().unwrap_or_default())
    }
}
```

Add `pub mod mock;` to `src/vectordb/mod.rs` (gated `#[cfg(test)]` if you prefer).

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib knowledge::semantic_backfill 2>&1 | tail -15`
Expected: Both semantic backfill tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/knowledge/semantic_backfill.rs src/knowledge/mod.rs src/store/mod.rs src/vectordb/
git commit -m "feat(p5): semantic backfill via LanceDB ANN

Adds semantic_backfill module: for each article, queries LanceDB for
top-K nearest neighbors and emits SEMANTICALLY_RELATED edges for those
above a configurable cosine threshold (default 0.85). Adds
list_article_ids on Store and a MockVectorDb for test isolation."
```

---

### Task 6: Citation backfill (markdown parsing, cheap, no LLM)

**Files:**
- Create: `src/knowledge/citation_backfill.rs`
- Modify: `src/knowledge/mod.rs`

- [ ] **Step 1: Write failing test for citation backfill**

Create `src/knowledge/citation_backfill.rs`:

```rust
//! Citation-edge backfill via markdown link parsing.
//!
//! Looks for `[anchor](article_id)` patterns in article content where
//! `article_id` matches an existing article in the same store. Emits a
//! REFERENCES_EDGE for each match. Cheap; idempotent; user-asserted.

use anyhow::Result;
use regex::Regex;

use crate::store::Store;

/// Per-store citation backfill. Returns the number of edges created.
pub async fn backfill_citations<S: Store + Sync>(store: &S, store_id: &str) -> Result<u64> {
    let re = Regex::new(r"\[([^\]]+)\]\(([a-zA-Z0-9_-]+)\)").expect("static regex compiles");

    let ids = store.list_article_ids(store_id).await?;
    let id_set: std::collections::HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
    let mut count = 0u64;

    for from_id in &ids {
        let Some(article) = store.get_article(store_id, from_id).await? else { continue };
        for cap in re.captures_iter(&article.content) {
            let anchor = cap.get(1).map(|m| m.as_str().to_string());
            let target = cap.get(2).map(|m| m.as_str().to_string());
            let Some(target) = target else { continue };
            if target == *from_id { continue; }
            if !id_set.contains(target.as_str()) { continue; }

            store.create_references_edge(store_id, from_id, &target, anchor).await?;
            count += 1;
        }
    }

    tracing::info!("Citation backfill complete for store {}: {} REFERENCES edges", store_id, count);
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SurrealStore;

    #[tokio::test]
    async fn citation_backfill_emits_edge_for_existing_target() {
        let store = SurrealStore::open_memory().await.expect("open mem");
        store.db().query(r#"
            CREATE article:src CONTENT { store_id: "s1", title: "S",
                content: "see [the retro](tgt) for context",
                source_type: "user", source_id: "", content_hash: "s", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:tgt CONTENT { store_id: "s1", title: "T", content: "",
                source_type: "user", source_id: "", content_hash: "t", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
        "#).await.expect("seed").check().expect("seed check");

        let n = backfill_citations(&store, "s1").await.expect("backfill");
        assert_eq!(n, 1);

        let edges = store.list_references_for("s1", "src").await.expect("list");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].anchor_text.as_deref(), Some("the retro"));
    }

    #[tokio::test]
    async fn citation_backfill_skips_unknown_targets() {
        let store = SurrealStore::open_memory().await.expect("open mem");
        store.db().query(r#"
            CREATE article:src CONTENT { store_id: "s1", title: "S",
                content: "see [missing](does_not_exist)",
                source_type: "user", source_id: "", content_hash: "s", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
        "#).await.expect("seed").check().expect("seed check");

        let n = backfill_citations(&store, "s1").await.expect("backfill");
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn citation_backfill_skips_self_references() {
        let store = SurrealStore::open_memory().await.expect("open mem");
        store.db().query(r#"
            CREATE article:src CONTENT { store_id: "s1", title: "S",
                content: "see [self](src)",
                source_type: "user", source_id: "", content_hash: "s", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
        "#).await.expect("seed").check().expect("seed check");

        let n = backfill_citations(&store, "s1").await.expect("backfill");
        assert_eq!(n, 0);
    }
}
```

Also add to `src/knowledge/mod.rs`:

```rust
pub mod citation_backfill;
```

If `regex` isn't already a dependency, add it to `Cargo.toml`:

```toml
regex = "1"
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib knowledge::citation_backfill 2>&1 | tail -10`
Expected: FAIL — module not yet created OR `regex` missing.

- [ ] **Step 3: Implementation already in Step 1 above — verify tests pass**

Run: `cargo test --lib knowledge::citation_backfill 2>&1 | tail -15`
Expected: All three citation backfill tests PASS.

- [ ] **Step 4: Commit**

```bash
git add src/knowledge/citation_backfill.rs src/knowledge/mod.rs Cargo.toml Cargo.lock
git commit -m "feat(p5): citation backfill via markdown link parsing

Adds citation_backfill module: scans article content for
[anchor](article_id) patterns matching known article ids in the same
store, emits REFERENCES_EDGE per match. Skips unknown targets and
self-references."
```

---

### Task 7: Causal extractor (LLM via Ollama, slow path)

**Files:**
- Create: `src/knowledge/causal_extractor.rs`
- Modify: `src/knowledge/mod.rs`
- Modify: `src/config/mod.rs`

- [ ] **Step 1: Add `[graph]` config section with causal subkey**

In `src/config/mod.rs`, after the existing `ExtractionConfig` struct, add:

```rust
/// Graph / multi-graph configuration (P5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphConfig {
    /// Cosine similarity threshold for SEMANTICALLY_RELATED edges
    /// (HippoRAG uses 0.8, SYNAPSE 0.92; KNL midpoint default).
    #[serde(default = "default_semantic_threshold")]
    pub semantic_threshold: f64,

    /// Top-K nearest neighbors to scan per article during semantic backfill.
    #[serde(default = "default_semantic_top_k")]
    pub semantic_top_k: usize,

    /// If true, attempt causal extraction during ingestion (LLM-bound).
    /// Default false: opt-in because expensive and there is no honest heuristic.
    #[serde(default)]
    pub causal_enabled: bool,

    /// Ollama model for causal extraction. Independent of `extraction.model`
    /// because causal extraction is sensitive to instruction-following
    /// (typically wants a larger model than entity extraction).
    #[serde(default = "default_causal_model")]
    pub causal_model: String,

    /// Confidence threshold below which causal edges are stored for audit
    /// only and excluded from retrieval traversal.
    #[serde(default = "default_causal_confidence_threshold")]
    pub causal_confidence_threshold: f64,
}

fn default_semantic_threshold() -> f64 { 0.85 }
fn default_semantic_top_k() -> usize { 20 }
fn default_causal_model() -> String { "llama3.2:3b".into() }
fn default_causal_confidence_threshold() -> f64 { 0.6 }

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            semantic_threshold: default_semantic_threshold(),
            semantic_top_k: default_semantic_top_k(),
            causal_enabled: false,
            causal_model: default_causal_model(),
            causal_confidence_threshold: default_causal_confidence_threshold(),
        }
    }
}
```

Then add `pub graph: GraphConfig,` (with `#[serde(default)]`) to the main `Config` struct.

- [ ] **Step 2: Write failing test for causal extraction parsing**

Create `src/knowledge/causal_extractor.rs`:

```rust
//! LLM-based causal-edge extraction via local Ollama.
//!
//! Given two article excerpts (a "source" article and a candidate "effect"
//! article), prompts the LLM to decide whether the source caused or enabled
//! the effect. Returns confidence and rationale. Mirrors the structure of
//! `EntityExtractor`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::GraphConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CausalClaim {
    /// 0.0 if the LLM judges no causal link; otherwise its confidence
    /// in the source → effect direction.
    pub confidence: f64,
    pub rationale: Option<String>,
}

pub struct CausalExtractor {
    config: GraphConfig,
    ollama_url: String,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    format: String,
    stream: bool,
}

#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
}

impl CausalExtractor {
    pub fn new(config: GraphConfig, ollama_url: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .expect("build reqwest client");
        Self { config, ollama_url, client }
    }

    fn build_prompt(source_title: &str, source_excerpt: &str, effect_title: &str, effect_excerpt: &str) -> String {
        format!(
            r#"You judge causal relationships between two events or claims described in short excerpts.

Determine whether SOURCE causes or enables EFFECT. Output a JSON object with:
- "confidence": a float in [0.0, 1.0]. 0.0 means no causal relationship; 1.0 means clearly causal.
- "rationale": a short string (<=200 chars) explaining your judgment.

Only output the JSON object. No prose.

SOURCE: {st}
SOURCE EXCERPT: {se}

EFFECT: {et}
EFFECT EXCERPT: {ee}"#,
            st = source_title, se = source_excerpt,
            et = effect_title, ee = effect_excerpt,
        )
    }

    /// Returns `None` if `causal_enabled` is false or the LLM call fails.
    pub async fn extract(&self, source_title: &str, source_excerpt: &str,
                         effect_title: &str, effect_excerpt: &str) -> Result<Option<CausalClaim>> {
        if !self.config.causal_enabled {
            return Ok(None);
        }

        let prompt = Self::build_prompt(source_title, source_excerpt, effect_title, effect_excerpt);
        let url = format!("{}/api/generate", self.ollama_url);

        let req = OllamaRequest {
            model: self.config.causal_model.clone(),
            prompt,
            format: "json".into(),
            stream: false,
        };

        let resp = self.client.post(&url).json(&req).send().await.context("ollama")?;
        if !resp.status().is_success() {
            anyhow::bail!("ollama HTTP {}", resp.status());
        }
        let body: OllamaResponse = resp.json().await.context("parse ollama body")?;
        Self::parse_response(&body.response).map(Some)
    }

    fn parse_response(json_str: &str) -> Result<CausalClaim> {
        let claim: CausalClaim = serde_json::from_str(json_str)
            .with_context(|| format!("parse causal claim from `{}`", json_str))?;
        // Clamp confidence to [0.0, 1.0]
        let clamped = claim.confidence.clamp(0.0, 1.0);
        Ok(CausalClaim { confidence: clamped, rationale: claim.rationale })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_response_extracts_confidence_and_rationale() {
        let json = r#"{"confidence": 0.78, "rationale": "explicit 'because' clause"}"#;
        let claim = CausalExtractor::parse_response(json).unwrap();
        assert!((claim.confidence - 0.78).abs() < 1e-9);
        assert_eq!(claim.rationale.as_deref(), Some("explicit 'because' clause"));
    }

    #[test]
    fn parse_response_clamps_confidence() {
        let json = r#"{"confidence": 1.5, "rationale": "very confident"}"#;
        let claim = CausalExtractor::parse_response(json).unwrap();
        assert_eq!(claim.confidence, 1.0);

        let json = r#"{"confidence": -0.2}"#;
        let claim = CausalExtractor::parse_response(json).unwrap();
        assert_eq!(claim.confidence, 0.0);
    }

    #[test]
    fn parse_response_handles_missing_rationale() {
        let json = r#"{"confidence": 0.4}"#;
        let claim = CausalExtractor::parse_response(json).unwrap();
        assert_eq!(claim.confidence, 0.4);
        assert!(claim.rationale.is_none());
    }

    #[test]
    fn parse_response_errors_on_bad_json() {
        let res = CausalExtractor::parse_response("not json");
        assert!(res.is_err());
    }

    #[test]
    fn build_prompt_contains_both_titles() {
        let p = CausalExtractor::build_prompt("S", "se", "E", "ee");
        assert!(p.contains("SOURCE: S"));
        assert!(p.contains("EFFECT: E"));
    }
}
```

Also add to `src/knowledge/mod.rs`:

```rust
pub mod causal_extractor;
```

Add `serde` derive for `CausalClaim` `Default`-tolerant if needed (the `#[serde(default)]` pattern from `ExtractedEntity` is the model).

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test --lib knowledge::causal_extractor 2>&1 | tail -15`
Expected: All five parsing tests PASS. (Network-bound `extract()` is not unit-tested; it's covered by the integration test in Task 11.)

- [ ] **Step 4: Commit**

```bash
git add src/knowledge/causal_extractor.rs src/knowledge/mod.rs src/config/mod.rs
git commit -m "feat(p5): causal extractor scaffolding via Ollama

Adds CausalExtractor mirroring EntityExtractor's HTTP pattern, plus a
GraphConfig section gating causal extraction behind an opt-in flag.
The actual edge emission is wired in Task 8 (relation_extractor)."
```

---

### Task 8: Relation extractor orchestrator + per-method dispatch

**Files:**
- Create: `src/knowledge/relation_extractor.rs`
- Modify: `src/knowledge/mod.rs`

- [ ] **Step 1: Write failing test for the orchestrator**

Create `src/knowledge/relation_extractor.rs`:

```rust
//! Orchestrates the four P5 backfill paths: temporal, semantic, citations,
//! causal. Each is independently toggleable; counts are returned per method.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::GraphConfig;
use crate::knowledge::{citation_backfill, semantic_backfill, temporal_backfill};
use crate::store::Store;
use crate::vectordb::VectorDb;

/// Which paths to run. All independent; any combination is valid.
#[derive(Debug, Clone, Default)]
pub struct ExtractRelationsRequest {
    pub temporal: bool,
    pub semantic: bool,
    pub citations: bool,
    pub causal: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtractRelationsReport {
    pub temporal_edges: u64,
    pub semantic_edges: u64,
    pub citation_edges: u64,
    pub causal_edges: u64,
}

pub async fn extract_relations<S: Store + Sync, V: VectorDb + Sync>(
    store: &S,
    vector_db: &V,
    config: &GraphConfig,
    store_id: &str,
    req: ExtractRelationsRequest,
) -> Result<ExtractRelationsReport> {
    let mut report = ExtractRelationsReport {
        temporal_edges: 0,
        semantic_edges: 0,
        citation_edges: 0,
        causal_edges: 0,
    };

    if req.temporal {
        report.temporal_edges = temporal_backfill::backfill_temporal(store, store_id).await?;
    }
    if req.semantic {
        report.semantic_edges = semantic_backfill::backfill_semantic(
            store, vector_db, store_id,
            config.semantic_threshold, config.semantic_top_k,
        ).await?;
    }
    if req.citations {
        report.citation_edges = citation_backfill::backfill_citations(store, store_id).await?;
    }
    if req.causal {
        // Causal backfill is a slow LLM path; implemented in Task 10 (CLI wiring).
        // For P5, run an LLM pass over each entity-overlap pair where both
        // articles share an entity. We bound work via the existing entity-overlap
        // graph; without that gate, causal extraction would be O(N²).
        report.causal_edges = run_causal_backfill(store, config, store_id).await?;
    }
    Ok(report)
}

async fn run_causal_backfill<S: Store + Sync>(
    store: &S,
    config: &GraphConfig,
    store_id: &str,
) -> Result<u64> {
    use crate::knowledge::causal_extractor::CausalExtractor;
    let extractor = CausalExtractor::new(config.clone(), "http://localhost:11434".into());

    let pairs = store.list_entity_overlap_pairs(store_id).await?;
    let mut count = 0u64;

    for (a_id, b_id) in pairs {
        let Some(a) = store.get_article(store_id, &a_id).await? else { continue };
        let Some(b) = store.get_article(store_id, &b_id).await? else { continue };

        let excerpt_a = truncate(&a.content, 600);
        let excerpt_b = truncate(&b.content, 600);

        if let Ok(Some(claim)) = extractor.extract(&a.title, excerpt_a, &b.title, excerpt_b).await {
            if claim.confidence >= config.causal_confidence_threshold {
                store.create_caused_by_edge(store_id, &a_id, &b_id, claim.confidence, claim.rationale).await?;
                count += 1;
            }
        }
    }

    Ok(count)
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max { s } else { &s[..max] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SurrealStore;
    use crate::vectordb::mock::MockVectorDb;

    #[tokio::test]
    async fn extract_relations_runs_only_requested_methods() {
        let store = SurrealStore::open_memory().await.expect("open mem");
        store.db().query(r#"
            CREATE article:a CONTENT { store_id: "s1", title: "A", content: "",
                source_type: "user", source_id: "", content_hash: "a", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:b CONTENT { store_id: "s1", title: "B", content: "",
                source_type: "user", source_id: "", content_hash: "b", tags: [],
                created_at: "2026-02-01T00:00:00Z", updated_at: "2026-02-01T00:00:00Z" };
            RELATE article:a->entity_overlap->article:b CONTENT {
                shared_entity_count: 1, strength: 0.3, confidence: 0.3,
                extraction_method: "heuristic", store_id: "s1",
                created_at: "2026-02-01T00:00:01Z", updated_at: "2026-02-01T00:00:01Z"
            };
        "#).await.expect("seed").check().expect("seed check");

        let cfg = GraphConfig::default(); // causal disabled by default
        let mock = MockVectorDb::with_pairs("s1", &[]);
        let req = ExtractRelationsRequest { temporal: true, semantic: false, citations: false, causal: false };

        let report = extract_relations(&store, &mock, &cfg, "s1", req).await.expect("extract");

        assert_eq!(report.temporal_edges, 1);
        assert_eq!(report.semantic_edges, 0);
        assert_eq!(report.citation_edges, 0);
        assert_eq!(report.causal_edges, 0);
    }
}
```

Also add to `src/knowledge/mod.rs`:

```rust
pub mod relation_extractor;
```

- [ ] **Step 2: Run test to verify it passes**

Run: `cargo test --lib knowledge::relation_extractor 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add src/knowledge/relation_extractor.rs src/knowledge/mod.rs
git commit -m "feat(p5): relation extractor orchestrator

Adds extract_relations entry point that dispatches to temporal, semantic,
citation, and causal backfills based on caller flags. Causal backfill is
gated by entity-overlap pairs to bound work (otherwise it would be O(N²)
LLM calls)."
```

---

### Task 9: Type-aware GraphSearcher integration

> **Note:** This task assumes P4's `src/retrieval/graph.rs` is merged. If not, fold the additions below into P4's implementation. P4's `GraphSearcher` traverses `MENTIONS` + `RELATED_TO`; P5 swaps `RELATED_TO` for `ENTITY_OVERLAP` and adds optional traversal of `SEMANTICALLY_RELATED`, `PRECEDES`, `CAUSED_BY`, `REFERENCES_EDGE` per the caller's `EdgeTypeFilter`.

**Files:**
- Modify: `src/retrieval/graph.rs`
- Modify: `src/config/mod.rs` (extend `RetrievalConfig`)

- [ ] **Step 1: Add `EdgeTypeFilter` to retrieval config**

In `src/config/mod.rs`, extend `RetrievalConfig` (added by P4):

```rust
/// Which edge types the graph signal should traverse. Defaults preserve P4
/// behavior (entity_overlap + entity-mention bridge only).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeTypeFilter {
    #[serde(default = "default_true")]
    pub entity_overlap: bool,
    #[serde(default)]
    pub semantically_related: bool,
    #[serde(default)]
    pub precedes: bool,
    #[serde(default)]
    pub caused_by: bool,
    #[serde(default)]
    pub references: bool,
}

fn default_true() -> bool { true }

impl Default for EdgeTypeFilter {
    fn default() -> Self {
        Self {
            entity_overlap: true,
            semantically_related: false,
            precedes: false,
            caused_by: false,
            references: false,
        }
    }
}
```

Then add `pub edge_types: EdgeTypeFilter,` (with `#[serde(default)]`) to the existing `RetrievalConfig` struct.

- [ ] **Step 2: Write a failing test that exercises edge-type-filtered traversal**

In `src/retrieval/graph.rs` (or its test module), add:

```rust
    #[tokio::test]
    async fn graph_searcher_traverses_only_enabled_edge_types() {
        let store = SurrealStore::open_memory().await.expect("open mem");
        store.db().query(r#"
            CREATE article:src CONTENT { store_id: "s1", title: "Src", content: "",
                source_type: "user", source_id: "", content_hash: "src", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:overlap_target CONTENT { store_id: "s1", title: "EO", content: "",
                source_type: "user", source_id: "", content_hash: "eo", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:sem_target CONTENT { store_id: "s1", title: "SEM", content: "",
                source_type: "user", source_id: "", content_hash: "sem", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            RELATE article:src->entity_overlap->article:overlap_target CONTENT {
                shared_entity_count: 2, strength: 0.5, confidence: 0.5,
                extraction_method: "heuristic", store_id: "s1",
                created_at: "2026-01-01T00:00:01Z", updated_at: "2026-01-01T00:00:01Z"
            };
            RELATE article:src->semantically_related->article:sem_target CONTENT {
                similarity: 0.9, confidence: 0.9,
                extraction_method: "derived", store_id: "s1",
                created_at: "2026-01-01T00:00:02Z"
            };
        "#).await.expect("seed").check().expect("seed check");

        // Default filter: only entity_overlap enabled
        let filter = EdgeTypeFilter::default();
        let searcher = GraphSearcher::new(&store, /* config */);
        let hits = searcher.search_neighbors("s1", "src", &filter, 1).await.expect("search");
        let titles: Vec<&str> = hits.iter().map(|h| h.article_id.as_str()).collect();
        assert!(titles.contains(&"overlap_target"), "entity_overlap target missing");
        assert!(!titles.contains(&"sem_target"), "semantically_related must be excluded when disabled");

        // Enable semantically_related
        let mut filter2 = EdgeTypeFilter::default();
        filter2.semantically_related = true;
        let hits2 = searcher.search_neighbors("s1", "src", &filter2, 1).await.expect("search2");
        let titles2: Vec<&str> = hits2.iter().map(|h| h.article_id.as_str()).collect();
        assert!(titles2.contains(&"overlap_target"));
        assert!(titles2.contains(&"sem_target"));
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test --lib retrieval::graph::tests::graph_searcher_traverses_only_enabled_edge_types 2>&1 | tail -10`
Expected: FAIL — `search_neighbors` not type-aware.

- [ ] **Step 4: Extend `GraphSearcher` to accept `EdgeTypeFilter`**

In `src/retrieval/graph.rs`, change `search_neighbors` (or whatever the P4 traversal function is called) to take `&EdgeTypeFilter` and build the UNION SELECT dynamically:

```rust
impl<'a, S: Store + Sync> GraphSearcher<'a, S> {
    pub async fn search_neighbors(
        &self,
        store_id: &str,
        article_id: &str,
        filter: &EdgeTypeFilter,
        max_hops: usize,
    ) -> Result<Vec<GraphHit>> {
        // Build the list of edge tables to traverse based on filter.
        // Each enabled type contributes a SELECT into the UNION.
        let mut union_parts: Vec<String> = Vec::new();

        if filter.entity_overlap {
            union_parts.push(format!(
                "SELECT meta::id(out) AS article_id, 'entity_overlap' AS edge_type,
                        confidence AS score
                 FROM entity_overlap WHERE store_id = $sid
                   AND in = type::thing('article', $aid)"
            ));
        }
        if filter.semantically_related {
            union_parts.push(format!(
                "SELECT meta::id(out) AS article_id, 'semantically_related' AS edge_type,
                        similarity AS score
                 FROM semantically_related WHERE store_id = $sid
                   AND in = type::thing('article', $aid)"
            ));
        }
        if filter.precedes {
            union_parts.push(format!(
                "SELECT meta::id(out) AS article_id, 'precedes' AS edge_type,
                        confidence AS score
                 FROM precedes WHERE store_id = $sid
                   AND in = type::thing('article', $aid)"
            ));
        }
        if filter.caused_by {
            union_parts.push(format!(
                "SELECT meta::id(out) AS article_id, 'caused_by' AS edge_type,
                        confidence AS score
                 FROM caused_by WHERE store_id = $sid
                   AND in = type::thing('article', $aid)"
            ));
        }
        if filter.references {
            union_parts.push(format!(
                "SELECT meta::id(out) AS article_id, 'references_edge' AS edge_type,
                        confidence AS score
                 FROM references_edge WHERE store_id = $sid
                   AND in = type::thing('article', $aid)"
            ));
        }

        if union_parts.is_empty() {
            return Ok(vec![]);
        }

        let union_query = union_parts.join(" UNION ");
        let mut resp = self.store.db()
            .query(&union_query)
            .bind(("sid", store_id.to_string()))
            .bind(("aid", article_id.to_string()))
            .await?;
        #[derive(serde::Deserialize)]
        struct Row { article_id: String, edge_type: String, score: f64 }
        let rows: Vec<Row> = resp.take(0).unwrap_or_default();

        let hits = rows.into_iter().map(|r| GraphHit {
            article_id: r.article_id,
            edge_type: r.edge_type,
            score: r.score,
            hop: 1,
        }).collect();

        // Multi-hop expansion: defer to P6 (activation). For P5, max_hops=1
        // is fine; higher values become hot-path drivers when activation lands.
        let _ = max_hops;
        Ok(hits)
    }
}

pub struct GraphHit {
    pub article_id: String,
    pub edge_type: String,
    pub score: f64,
    pub hop: usize,
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test --lib retrieval::graph 2>&1 | tail -15`
Expected: type-filtered traversal test PASSES; existing P4 tests still PASS (default filter preserves P4 entity_overlap-only behavior — but note P4 used `RELATED_TO`; if P4 tests reference `related_to`, update them to expect `entity_overlap` via the migration).

- [ ] **Step 6: Commit**

```bash
git add src/retrieval/graph.rs src/config/mod.rs
git commit -m "feat(p5): edge-type-aware GraphSearcher traversal

Generalizes GraphSearcher::search_neighbors to take an EdgeTypeFilter
config that selects which P5 edge tables to UNION into the traversal.
Default filter preserves P4 behavior (entity_overlap only). Multi-hop
expansion deferred to P6 activation."
```

---

### Task 10: CLI subcommand `extract-relations`

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Add the subcommand**

In `src/main.rs`, where existing subcommands are defined (look for the `Commands` enum derived from `clap::Subcommand`), add:

```rust
    /// Run P5 multi-graph backfill: temporal, semantic, citation, causal.
    ExtractRelations {
        /// Knowledge store id (required).
        #[arg(long)]
        store_id: String,

        #[arg(long)]
        temporal: bool,

        #[arg(long)]
        semantic: bool,

        #[arg(long)]
        citations: bool,

        #[arg(long)]
        causal: bool,

        /// Run all four (equivalent to --temporal --semantic --citations --causal).
        #[arg(long)]
        all: bool,
    },
```

In the `main` dispatch:

```rust
        Commands::ExtractRelations { store_id, temporal, semantic, citations, causal, all } => {
            let store = open_store(&config).await?;
            let vector_db = open_vector_db(&config).await?;

            let req = ExtractRelationsRequest {
                temporal: temporal || all,
                semantic: semantic || all,
                citations: citations || all,
                causal: causal || all,
            };

            let report = extract_relations(&store, &vector_db, &config.graph, &store_id, req).await?;
            println!("P5 backfill complete:");
            println!("  temporal:  {} edges", report.temporal_edges);
            println!("  semantic:  {} edges", report.semantic_edges);
            println!("  citations: {} edges", report.citation_edges);
            println!("  causal:    {} edges", report.causal_edges);
        }
```

- [ ] **Step 2: Extend `graph-debug` with per-edge-type counts**

In the existing `graph-debug` subcommand handler in `src/main.rs`, add (after whatever it currently prints):

```rust
            // P5 per-edge-type counts
            let counts = store.count_edges_by_type(&store_id).await?;
            println!();
            println!("P5 edge counts ({}):", store_id);
            println!("  entity_overlap:        {}", counts.entity_overlap);
            println!("  semantically_related:  {}", counts.semantically_related);
            println!("  precedes:              {}", counts.precedes);
            println!("  caused_by:             {}", counts.caused_by);
            println!("  references_edge:       {}", counts.references_edge);
```

Add the `count_edges_by_type` helper on `Store`:

```rust
    async fn count_edges_by_type(&self, store_id: &str) -> Result<EdgeCounts>;

pub struct EdgeCounts {
    pub entity_overlap: i64,
    pub semantically_related: i64,
    pub precedes: i64,
    pub caused_by: i64,
    pub references_edge: i64,
}
```

`SurrealStore` impl:

```rust
    async fn count_edges_by_type(&self, store_id: &str) -> Result<EdgeCounts> {
        async fn count_one(db: &Surreal<Any>, table: &str, sid: &str) -> Result<i64> {
            let q = format!("SELECT count() AS n FROM {} WHERE store_id = $sid GROUP ALL", table);
            let mut resp = db.query(&q).bind(("sid", sid.to_string())).await?;
            #[derive(serde::Deserialize)] struct C { n: i64 }
            let rows: Vec<C> = resp.take(0).unwrap_or_default();
            Ok(rows.first().map(|c| c.n).unwrap_or(0))
        }

        Ok(EdgeCounts {
            entity_overlap:       count_one(&self.db, "entity_overlap", store_id).await?,
            semantically_related: count_one(&self.db, "semantically_related", store_id).await?,
            precedes:             count_one(&self.db, "precedes", store_id).await?,
            caused_by:            count_one(&self.db, "caused_by", store_id).await?,
            references_edge:      count_one(&self.db, "references_edge", store_id).await?,
        })
    }
```

- [ ] **Step 3: Build and smoke-test the CLI**

Run: `cargo build 2>&1 | tail -5`
Expected: builds cleanly.

Run: `cargo run -- --help 2>&1 | grep -i extract-relations`
Expected: the new subcommand appears in help output.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs src/store/mod.rs
git commit -m "feat(p5): extract-relations CLI and per-edge-type debug counts

Adds the extract-relations subcommand with --temporal, --semantic,
--citations, --causal, --all flags. Extends graph-debug with per-edge-type
counts via Store::count_edges_by_type. Causal extraction requires
[graph].causal_enabled in config."
```

---

### Task 11: End-to-end integration test on a fixture corpus

**Files:**
- Create: `tests/p5_multi_graph_e2e.rs`

- [ ] **Step 1: Write the integration test**

Create `tests/p5_multi_graph_e2e.rs`:

```rust
//! Integration test for P5 multi-graph backfill.
//!
//! Seeds a 10-article fixture corpus with deterministic timestamps,
//! pre-computed entity-overlap edges, controlled embeddings, and a markdown
//! link. Runs each backfill path (temporal, semantic, citations) and
//! asserts the expected number of edges. Causal extraction is skipped
//! because it requires a running Ollama instance.

use knowledge_nexus_local::config::GraphConfig;
use knowledge_nexus_local::knowledge::relation_extractor::{
    extract_relations, ExtractRelationsRequest,
};
use knowledge_nexus_local::store::SurrealStore;
use knowledge_nexus_local::vectordb::mock::MockVectorDb;

#[tokio::test]
async fn p5_backfill_emits_expected_edges_on_10_article_fixture() {
    let store = SurrealStore::open_memory().await.expect("open mem store");

    // Seed 10 articles with deterministic timestamps in 2026-01-01..2026-01-10
    let mut seed_q = String::new();
    for i in 1..=10 {
        seed_q.push_str(&format!(
            "CREATE article:a{i} CONTENT {{ store_id: \"s1\", title: \"A{i}\",
                content: \"{content}\",
                source_type: \"user\", source_id: \"\", content_hash: \"a{i}\",
                tags: [], created_at: \"2026-01-{i:02}T00:00:00Z\",
                updated_at: \"2026-01-{i:02}T00:00:00Z\" }};\n",
            i = i,
            content = if i == 5 { "see [retro](a3) for the prior incident".to_string() }
                      else { String::new() },
        ));
    }
    // 3 entity-overlap pairs: (a1,a2), (a3,a5), (a7,a8)
    for (a, b) in &[(1, 2), (3, 5), (7, 8)] {
        seed_q.push_str(&format!(
            "RELATE article:a{a}->entity_overlap->article:a{b} CONTENT {{
                shared_entity_count: 2, strength: 0.5, confidence: 0.5,
                extraction_method: \"heuristic\", store_id: \"s1\",
                created_at: \"2026-01-10T00:00:00Z\", updated_at: \"2026-01-10T00:00:00Z\"
             }};\n", a = a, b = b
        ));
    }
    store.db().query(&seed_q).await.expect("seed").check().expect("seed check");

    // MockVectorDb: a2 ↔ a3 are very similar (0.95); everyone else <0.6
    let mock = MockVectorDb::with_pairs("s1", &[
        ("a1", &[("a2", 0.5), ("a3", 0.5)]),
        ("a2", &[("a3", 0.95), ("a1", 0.5)]),
        ("a3", &[("a2", 0.95), ("a5", 0.5)]),
        ("a4", &[("a5", 0.5)]),
        ("a5", &[("a3", 0.5)]),
        ("a6", &[("a7", 0.5)]),
        ("a7", &[("a8", 0.5), ("a6", 0.5)]),
        ("a8", &[("a7", 0.5)]),
        ("a9", &[("a10", 0.5)]),
        ("a10", &[("a9", 0.5)]),
    ]);

    let cfg = GraphConfig { semantic_threshold: 0.85, semantic_top_k: 5, ..Default::default() };
    let req = ExtractRelationsRequest {
        temporal: true, semantic: true, citations: true, causal: false,
    };
    let report = extract_relations(&store, &mock, &cfg, "s1", req).await.expect("extract");

    // Temporal: 3 PRECEDES edges, one per entity_overlap pair
    assert_eq!(report.temporal_edges, 3, "expected 3 temporal edges from 3 overlap pairs");

    // Semantic: a2-a3 pair (one edge after directional dedup)
    assert_eq!(report.semantic_edges, 2, "two calls (a2->a3, a3->a2) but unique index keeps one row");

    // Citations: a5 -> a3 (one explicit markdown link)
    assert_eq!(report.citation_edges, 1);

    // Verify edge counts via Store::count_edges_by_type
    let counts = store.count_edges_by_type("s1").await.expect("counts");
    assert_eq!(counts.entity_overlap, 3, "seeded entity_overlap edges unchanged");
    assert_eq!(counts.precedes, 3);
    assert_eq!(counts.semantically_related, 1, "one unique semantic edge after dedup");
    assert_eq!(counts.caused_by, 0);
    assert_eq!(counts.references_edge, 1);
}
```

- [ ] **Step 2: Run the integration test**

Run: `cargo test --test p5_multi_graph_e2e 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 3: Run the full test suite to catch any regressions**

Run: `cargo test 2>&1 | tail -20`
Expected: all tests pass (P1, P2, P3, P4 tests should be unaffected by P5).

- [ ] **Step 4: Commit**

```bash
git add tests/p5_multi_graph_e2e.rs
git commit -m "test(p5): end-to-end multi-graph backfill on 10-article fixture

Integration test that seeds a 10-article corpus with controlled
entity-overlap pairs, embeddings, and a markdown citation, then runs
the temporal/semantic/citation backfills together and asserts the
expected per-edge-type counts."
```

---

### Task 12: Open a pull request

**Files:** none

- [ ] **Step 1: Verify branch is feature-named per CLAUDE.md conventions**

Run: `git branch --show-current`
Expected: `feat/p5-multi-graph` (or similar). If on `main`, switch:

```bash
git checkout -b feat/p5-multi-graph
```

- [ ] **Step 2: Push the branch**

```bash
git push -u origin feat/p5-multi-graph
```

- [ ] **Step 3: Open the PR**

```bash
gh pr create --title "P5: Decoupled Multi-Graph (entity-overlap, semantic, temporal, causal, citation edges)" --body "$(cat <<'EOF'
## Summary
- Replaces P3's single `RELATED_TO` edge with five typed edges: `entity_overlap` (renamed), `semantically_related` (NEW, cos > 0.85), `precedes` (NEW, deterministic from timestamps), `caused_by` (NEW, LLM via Ollama, opt-in), `references_edge` (NEW, parsed from markdown links).
- Per-graph backfill prioritized by cost: temporal (SQL only) → semantic (LanceDB ANN) → citations (regex parse) → causal (LLM, opt-in).
- Migration renames `related_to` → `entity_overlap` preserving all P3 data with `extraction_method='heuristic'`.
- `GraphSearcher` (from P4) becomes edge-type-aware via `EdgeTypeFilter`; default filter preserves P4 behavior.
- New `extract-relations` CLI subcommand with `--temporal/--semantic/--citations/--causal/--all` flags.

## Test plan
- [ ] `cargo test --lib store::` — schema, models, migrations, typed-edge CRUD all pass
- [ ] `cargo test --lib knowledge::` — temporal/semantic/citation/causal backfills + orchestrator all pass
- [ ] `cargo test --lib retrieval::graph` — edge-type-aware traversal passes
- [ ] `cargo test --test p5_multi_graph_e2e` — 10-article fixture e2e passes
- [ ] `cargo build --release` — release build clean
- [ ] Manual smoke test: `cargo run -- extract-relations --store-id <id> --all` on a real corpus; verify counts via `cargo run -- graph-debug --store-id <id>`

## References
- Plan: `docs/superpowers/plans/2026-05-23-p5-multi-graph.md`
- Roadmap: `docs/superpowers/plans/2026-05-23-supermemory-upgrade-roadmap.md` (P5 section)
EOF
)"
```

Return the PR URL to the user.

---

## Plan Self-Review

**Spec coverage:** Each P5 section in the roadmap maps to a task here:
- Schema additions + edge metadata → Task 1
- Migration (rename `related_to`) → Task 2
- Typed-edge CRUD → Task 3
- Temporal backfill (free) → Task 4
- Semantic backfill (cheap) → Task 5
- Citation backfill (cheap) → Task 6
- Causal extraction (LLM) → Tasks 7 + 8
- Type-aware traversal → Task 9
- CLI surface + debug counts → Task 10
- E2E integration test → Task 11
- PR workflow per CLAUDE.md → Task 12

**Type consistency:** `ExtractionMethod` is used identically across all edge structs and migration data. `EdgeTypeFilter` field names match `GraphSearcher::search_neighbors` query construction. Edge table names (`entity_overlap`, `semantically_related`, `precedes`, `caused_by`, `references_edge`) are spelled consistently in DDL, queries, model rows, and CLI output.

**No placeholders:** Every step has actual SurrealQL, Rust code, or shell commands. The two assumptions made explicit: (a) P4 is merged before P5 starts (header note + Task 9 guidance); (b) the `Store` trait already has `get_article` and `db()` accessors from P1/P3 — if not, thin shims are noted inline.

**Out-of-scope (explicitly deferred to P6+):** multi-hop traversal in `GraphSearcher` (P5's `max_hops` parameter is a no-op until activation lands); per-edge-type weight tuning (uses uniform defaults); intent-adaptive policy (P6); spreading activation (P6); decay / tiering (P8).
