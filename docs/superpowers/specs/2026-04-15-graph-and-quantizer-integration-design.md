# Graph + Pluggable Vector Quantizer Integration — 1.0.0 Design

**Date:** 2026-04-15
**Target release:** knowledge-nexus-local 1.0.0
**Related work:** Track A (`turboquant-rs` standalone crate) — spec separately

## 1. Goals

1. Add native graph capability to knowledge-nexus-local across three use cases:
   - **Knowledge graph from content** — entity extraction from articles, entity↔entity and entity↔article edges to power multi-hop RAG.
   - **Lightweight relationships** — article↔article, article↔tag, conversation↔article edges.
   - **Federation/agent mesh** — nodes, trust, federation, and discovery modeled as a graph.
2. Preserve all-native-Rust, fully-offline, embedded-library posture — no servers, no remote dependencies for core functionality.
3. Preserve vector-search quality and the path to state-of-the-art quantization by keeping LanceDB as the vector engine.
4. Introduce a pluggable `VectorQuantizer` trait so Track A's `turboquant-rs` crate can land as a first-class quantizer in 1.0.0 alongside existing IVF_PQ and int8 implementations.
5. Add article dedup and entity dedup so the graph stays clean when the same content arrives from multiple sources or surface forms.

## 2. Non-goals (1.0.0)

- No UI for browsing the graph.
- No GLiNER / zero-shot NER (pluggable extractor trait only — default is `bert-base-NER`).
- No entity embedding over full entity descriptions (only canonical-name embedding for dedup).
- No manual user "create edge" UI — all edges in 1.0.0 are derived from content, metadata, or federation state.
- No LLM-based entity disambiguation.
- No SIMD/GPU optimization pass on TurboQuant encode/decode — pure CPU in 1.0.0.
- No public benchmarks blog post — a marketing deliverable, coordinated separately.
- No backward-compatible dual-backend mode. 1.0.0 requires one-shot migration from the 0.8 SQLite database or a fresh install.

## 3. Background: current state (0.8)

```
┌─────────────────────────────────────────────────────────┐
│  knowledge-nexus-agent 0.8                              │
│  ┌────────────────┐  ┌──────────────┐  ┌─────────────┐  │
│  │  SQLite        │  │  SQLite FTS5 │  │  LanceDB    │  │
│  │  - relational  │  │  - articles  │  │  - vectors  │  │
│  │  - federation  │  │              │  │  (IVF_PQ)   │  │
│  └────────────────┘  └──────────────┘  └─────────────┘  │
│           └────── hybrid search (RRF) ──────┘           │
└─────────────────────────────────────────────────────────┘
```

Storage today:
- **SQLite** (`rusqlite`, bundled): relational tables (`users`, `knowledge_stores`, `articles`, `conversations`, `messages`, `k2k_clients`, `federation_agreements`, `discovered_nodes`, `connector_configs`) + FTS5 virtual table over `articles`.
- **LanceDB 0.13**: per-store vector collections, IVF_PQ quantization, 384-dim embeddings from `all-MiniLM-L6-v2` via ONNX Runtime (`ort` crate).
- **Hybrid retrieval**: RRF over LanceDB vectors + SQLite FTS5 at `src/retrieval/hybrid.rs`.

No graph engine. No entity extraction. No article or entity dedup. Tags stored as JSON arrays on `articles`.

## 4. Architecture (1.0.0)

```
┌───────────────────────────────────────────────────────────────────┐
│  knowledge-nexus-agent 1.0.0                                      │
│  ┌──────────────────────────────┐  ┌───────────────────────────┐  │
│  │  SurrealDB (embedded)        │  │  LanceDB                  │  │
│  │  - relational                │  │  - vectors via            │  │
│  │  - FTS                       │  │    VectorQuantizer trait  │  │
│  │  - graph (RELATE)            │  │    ├─ IVF_PQ (default)    │  │
│  │                              │  │    ├─ Int8 SQ             │  │
│  │  entities, federation mesh,  │  │    └─ TurboQuant (Track A)│  │
│  │  article links, tags         │  │                           │  │
│  └──────────────────────────────┘  └───────────────────────────┘  │
│              └──── hybrid search (graph + FTS + vector) ────┘     │
│                                                                   │
│  New: entity extraction at ingestion (local ONNX NER)             │
│  New: article dedup (content hash + near-duplicate embedding)     │
│  New: entity dedup (string canonicalization + embedding cluster)  │
└───────────────────────────────────────────────────────────────────┘
```

**Key architectural decisions:**

1. **SurrealDB replaces SQLite + FTS5** via the `surrealdb` crate with the `kv-surrealkv` pure-Rust backend (no RocksDB C++ dep).
2. **LanceDB stays** as the vector store. A new `VectorQuantizer` trait abstracts the quantization strategy; `TurboQuantQuantizer` (wrapping Track A's crate) ships alongside the existing IVF_PQ impl and a new int8 SQ impl.
3. **Entity extraction at ingestion.** Default extractor is `bert-base-NER` (4-class: PERSON, ORG, LOCATION, MISC) via `ort`. The extractor is a trait so GLiNER or custom models slot in post-1.0.0.
4. **Hybrid search becomes tri-signal:** FTS (SurrealDB) + vector (LanceDB) + graph expansion (SurrealDB 2-hop `MENTIONS` traversal), merged via RRF.
5. **Federation/agent mesh is graph-native.** `discovered_node`, `k2k_client`, and `federation_agreement` records connect via typed edges (`FEDERATES_WITH`, `TRUSTS`).
6. **Dedup is first-class.** Article and entity dedup run at ingestion, enabled by default, thresholds configurable per-store.

## 5. Data model

### 5.1 SurrealDB tables (relational)

Migrated from SQLite, SCHEMAFULL mode. Ownership and containment stay as foreign-key fields (1-to-many; no benefit from graph modeling).

| Table | Key fields |
|---|---|
| `user` | id, username, display_name, is_owner, settings, created_at, updated_at |
| `knowledge_store` | id, owner_id, store_type, name, lancedb_collection, quantizer_version, created_at, updated_at |
| `article` | id, store_id, title, content, source_type, source_id, content_hash, embedded_at, created_at, updated_at |
| `conversation` | id, user_id, title, message_count, created_at, updated_at |
| `message` | id, conversation_id, role, content, metadata, created_at |
| `k2k_client` | client_id, public_key_pem, client_name, status, registered_at |
| `federation_agreement` | id, local_store_id, remote_node_id, remote_endpoint, access_type, created_at |
| `discovered_node` | node_id, host, port, endpoint, capabilities, last_seen, healthy |
| `connector_config` | id, connector_type, name, config, store_id, created_at, updated_at |

**New tables (normalized from JSON or introduced):**

| Table | Fields |
|---|---|
| `entity` | id, canonical_name, aliases[], entity_type, canonical_entity_id (self-FK for merged entities), created_at |
| `tag` | id, name |

**New/changed fields on `article`:**
- `content_hash: text` — SHA-256 of normalized content, indexed, for exact-dedup.
- `source_id: text` — stable identifier from the source connector (URL, RSS GUID, etc.), preserves provenance across dedup.

`knowledge_store.quantizer_version` is new and persists which `VectorQuantizer` impl a store uses.

### 5.2 SurrealDB edges (graph)

All via SurrealDB `RELATE`. These are where the graph model earns its keep.

| Edge | Direction | Fields | Use case |
|---|---|---|---|
| `MENTIONS` | article → entity | confidence, offsets[] | KG-from-content |
| `TAGGED` | article → tag | — | normalized tagging |
| `RELATED_TO` | article → article | strength, source (manual\|derived) | lightweight links |
| `RELATED_TO` | entity → entity | strength, source (co-occurrence\|derived) | KG traversal |
| `DISCUSSED` | conversation → article | turn_id, confidence | RAG provenance |
| `DUPLICATE_OF` | article → article | source_connector, detected_at, similarity, method ("hash"\|"embedding") | article dedup |
| `FEDERATES_WITH` | discovered_node → discovered_node | agreement_id | federation mesh |
| `TRUSTS` | discovered_node → k2k_client | granted_at | trust graph |

### 5.3 LanceDB schema

Largely unchanged. One addition:

```
collection: <store_id>
  - article_id: text
  - chunk_id: text
  - chunk_text: text
  - embedding: vec<f32, 384>
  - quantizer_version: text    // NEW: "ivf_pq_v1" | "int8_v1" | "turboquant_v1"
  - metadata: json
```

Existing collections default to `ivf_pq_v1` after migration. New collections can be created with any supported quantizer. The `quantizer_version` field allows collections with mixed legacy/new data during transitions, with query-time dispatch.

## 6. Components and key traits

### 6.1 Module reshape

```
src/
├── store/              [RENAMED from `db/`] SurrealDB repository layer
│   ├── mod.rs          Store trait + SurrealStore impl
│   ├── schema.rs       Table + edge definitions, schemafull DDL
│   ├── migrations.rs   Version-gated schema migrations (SurrealDB-native)
│   └── models.rs       Rust types for tables + edges
├── vectordb/           LanceDB wrapper
│   ├── mod.rs
│   └── quantizer.rs    [NEW] VectorQuantizer trait + IvfPqQuantizer, Int8Quantizer, TurboQuantQuantizer
├── retrieval/          Hybrid: now tri-signal
│   ├── mod.rs
│   ├── hybrid.rs       RRF over vector + FTS + graph
│   └── graph_expand.rs [NEW] 2-hop MENTIONS traversal
├── knowledge/
│   ├── articles.rs
│   ├── conversations.rs
│   ├── entities.rs     [NEW] Entity resolution + dedup
│   ├── dedup.rs        [NEW] ArticleDeduplicator trait + HashPlusEmbeddingDedup impl
│   └── extraction/
│       ├── mod.rs      EntityExtractor trait
│       └── bert_ner.rs Default impl
├── federation/
├── k2k/
└── ...
```

### 6.2 `VectorQuantizer` trait (Track A integration seam)

```rust
// src/vectordb/quantizer.rs
pub trait VectorQuantizer: Send + Sync {
    /// Unique identifier stored in LanceDB metadata for dispatch.
    fn version(&self) -> &'static str;

    /// Train quantizer on a sample of vectors (for data-dependent quantizers).
    /// TurboQuant is training-free, so this is a no-op for it.
    fn train(&mut self, samples: &[Vec<f32>]) -> Result<()>;

    /// Encode a batch of vectors to their quantized byte representation.
    fn encode_batch(&self, vectors: &[Vec<f32>]) -> Result<Vec<Vec<u8>>>;

    /// Decode quantized bytes back to approximate f32 vectors.
    fn decode_batch(&self, codes: &[Vec<u8>]) -> Result<Vec<Vec<f32>>>;

    /// ANN search: find nearest-neighbor article_ids for a query vector.
    fn search(
        &self,
        query: &[f32],
        top_k: usize,
        filter: Option<&QuantizerFilter>,
    ) -> Result<Vec<(String, f32)>>;
}
```

**Impls shipped in 1.0.0:**
- `IvfPqQuantizer` — wraps LanceDB's native IVF_PQ (existing behavior; default for migrated stores).
- `Int8Quantizer` — wraps LanceDB's scalar quantization.
- `TurboQuantQuantizer` — bridges the `turboquant-rs` crate (Track A) into the trait.

Dispatch: `knowledge_store.quantizer_version` + `collection.quantizer_version` determine which impl runs at query time.

### 6.3 `EntityExtractor` trait

```rust
// src/knowledge/extraction/mod.rs
pub trait EntityExtractor: Send + Sync {
    fn name(&self) -> &'static str;

    async fn extract(
        &self,
        text: &str,
        store_config: &ExtractorConfig,
    ) -> Result<Vec<ExtractedEntity>>;
}

pub struct ExtractedEntity {
    pub canonical_name: String,
    pub entity_type: String,   // "PERSON", "ORG", etc. Stored as String for extensibility.
    pub confidence: f32,
    pub offsets: Vec<(usize, usize)>,
}
```

**Impl shipped in 1.0.0:** `BertNerExtractor` — loads `dslim/bert-base-NER` ONNX via `ort`.
**Pluggable for future:** `GlinerExtractor` (zero-shot, reads `ExtractorConfig.entity_types`).

### 6.4 `EntityDeduplicator` trait

```rust
pub trait EntityDeduplicator: Send + Sync {
    async fn find_matches(
        &self,
        candidate: &ExtractedEntity,
        existing: &[EntityRef],
    ) -> Result<Option<EntityRef>>;
}
```

**Default impl:** `StringPlusEmbeddingDedup`:
1. String canonicalization: lowercase + strip punctuation + alias list → bucket. Same-bucket entities of the same type merge.
2. Embedding similarity: embed canonical names via the existing MiniLM ONNX model. Within the same entity type, merge entities whose cosine similarity ≥ threshold (default **0.85**).

All `MENTIONS` edges resolve to the canonical entity. Merged surface forms are preserved in `entity.aliases`.

### 6.5 `ArticleDeduplicator` trait

```rust
pub trait ArticleDeduplicator: Send + Sync {
    async fn check(
        &self,
        incoming: &IncomingArticle,
        store_id: &str,
    ) -> Result<DedupDecision>;
}

pub enum DedupDecision {
    Novel,
    Duplicate {
        of: ArticleId,
        method: DedupMethod,
        similarity: f32,
    },
}
```

**Default impl:** `HashPlusEmbeddingDedup`:
1. Compute `content_hash = sha256(normalize(content))`. If hash match in same store exists → `Duplicate` with `method = Hash`.
2. Otherwise proceed through chunking + embedding. Before NER and graph writes, compare first-chunk embedding to existing articles' first-chunk embeddings. If cosine similarity ≥ threshold (default **0.92**) → `Duplicate` with `method = Embedding`.

On `Duplicate`, pipeline creates a `DUPLICATE_OF` edge pointing to the surviving article with the duplicate's `source_connector` and `source_id` preserved. No new embeddings or `MENTIONS` edges are created.

### 6.6 `Store` trait

Abstraction over SurrealDB. Single impl (`SurrealStore`) in 1.0.0. Exists primarily for testability via fakes.

```rust
pub trait Store: Send + Sync {
    // Relational
    async fn upsert_article(&self, article: &Article) -> Result<()>;
    async fn get_article(&self, id: &str) -> Result<Option<Article>>;
    async fn find_article_by_hash(&self, store_id: &str, hash: &str) -> Result<Option<Article>>;

    // FTS
    async fn fts_search(&self, query: &str, limit: usize) -> Result<Vec<Article>>;

    // Graph
    async fn relate(
        &self,
        from: &RecordId,
        edge: EdgeType,
        to: &RecordId,
        props: Option<Json>,
    ) -> Result<()>;
    async fn traverse(
        &self,
        from: &RecordId,
        edge: EdgeType,
        depth: usize,
    ) -> Result<Vec<RecordId>>;

    // ...
}
```

## 7. Ingestion pipeline

Steps 6–10 stage pending writes in memory; the SurrealDB transaction in step 11 performs all writes atomically (article record, entity records, edges). Nothing is visible in SurrealDB until step 11 commits.

```
Article ingested
  ├─ 1. Compute content_hash → exact-duplicate check (read-only query)
  │     └─ if duplicate: create DUPLICATE_OF edge in a small transaction, return. STOP.
  ├─ 2. Chunk content
  ├─ 3. Embed chunks (ONNX)
  ├─ 4. Near-duplicate check via first-chunk embedding (read-only query)
  │     └─ if duplicate: create DUPLICATE_OF edge, discard embeddings, return. STOP.
  ├─ 5. Quantize + write vectors to LanceDB (via active VectorQuantizer)
  ├─ 6. Run NER → ExtractedEntity[]
  ├─ 7. Entity dedup (string + embedding) → canonical entity resolution
  │     (reads existing entities; stages "create new" / "reuse existing" decisions)
  ├─ 8. Stage entity records (new + updated aliases on existing ones)
  ├─ 9. Stage MENTIONS edges (pointing at canonical entity ids — new or existing)
  ├─ 10. Stage TAGGED edges from normalized tags
  └─ 11. SurrealDB transaction: upsert article, upsert entities, create edges.
        Atomic — all or nothing.
```

LanceDB writes (step 5) are **not** part of the SurrealDB transaction. If the SurrealDB transaction at step 11 fails after LanceDB writes, the vectors reference an `article_id` that does not exist in SurrealDB. Orphaned vectors are cleaned up by a periodic reconciliation task that scans LanceDB `article_id`s against SurrealDB and removes rows with no matching article record.

## 8. Hybrid retrieval

Current (0.8):
```
merge(vector_results, fts_results) → top_k
```

1.0.0:
```
let vector_hits = vectordb.search_with_active_quantizer(query_vec, k);
let fts_hits    = store.fts_search(query_text, k);
let graph_hits  = graph_expand::from_seeds(
    &store,
    &[vector_hits.clone(), fts_hits.clone()].concat(),
    depth = 2,
);

rrf_merge(
    [vector_hits, fts_hits, graph_hits],
    weights = [1.0, 1.1, 0.7],  // graph boosts but doesn't dominate
    top_k,
)
```

**Graph expansion:** from the top-k vector/FTS seeds, traverse `MENTIONS → entity → MENTIONS` (2 hops) to surface topically-related articles. Boost their RRF score with weight **0.7** (below FTS/vector). Weights become tunable config.

**Graph depth default = 2.** 3 hops tends to blow up fan-out and hurt precision. Configurable.

## 9. Migration strategy

0.8 → 1.0.0 is a one-shot migration. No dual-backend mode.

```
knowledge-nexus-agent migrate --from sqlite --to surrealdb
```

Steps:
1. Open legacy SQLite DB read-only.
2. Create fresh SurrealDB DB in a new directory (SQLite untouched).
3. Stream each table → SurrealDB in batches of 1,000.
4. Construct `tag` records from articles' JSON-array `tags`.
5. Build `TAGGED` edges.
6. Backfill `content_hash` for each existing article (normalizes and hashes).
7. Leave `entity` records and `MENTIONS` edges unpopulated — generated by:
   ```
   knowledge-nexus-agent reindex --entities
   ```
8. Leave LanceDB untouched. Collections continue to use `ivf_pq_v1`. To upgrade a store's quantizer:
   ```
   knowledge-nexus-agent reindex --quantizer turboquant_v1 --store <id>
   ```
9. Write `migration_completed` marker file in data dir.
10. On next `start`, agent detects SurrealDB + marker → normal startup. If neither exists, clear error directs user to `migrate`.

Migration is idempotent at the row level: each record is written via an upsert keyed on the legacy primary key. A failure mid-run leaves partial data in the target SurrealDB DB but no `migration_completed` marker. Re-running without `--force` refuses to start if target has any data; re-running with `--force` upserts the full dataset and converges. SQLite is never modified.

`rusqlite` remains a dependency for the migration window; removed from main deps after 1.0.0 releases.

## 10. Error handling

| Failure | Behavior |
|---|---|
| NER failure on an article | Log warning. Skip entity extraction for that article. **Still write article + embeddings + FTS.** Article remains searchable via vector + FTS. |
| Quantizer encode failure | Fail article ingestion loudly. Vector search is core, not enrichment. |
| SurrealDB transaction failure | Fail loud. Atomic — no partial commits. |
| LanceDB write failure after SurrealDB write | Mark article with `embedded_at = null`. Background reconciliation retries. |
| Graph expansion query timeout (retrieval) | Log. Return vector + FTS results only. Hybrid search must never fail entirely due to graph. |
| Dedup check failure | Treat incoming as Novel (fail-open). Better to risk a duplicate than silently drop content. Log for operator review. |
| Migration failure mid-run | Target SurrealDB DB left in-place with no `migration_completed` marker. Operator re-runs with `--force` after addressing cause; idempotent upserts re-converge. SQLite is never modified. |

Concretely: introduce `ExtractionResult` and `IngestResult` types carrying per-component status so the pipeline records "article written, entities failed" without rolling back the article.

## 11. Testing

| Layer | Approach |
|---|---|
| `Store` trait | In-memory fake for unit tests; integration tests against embedded SurrealDB in `kv-mem` mode. |
| `VectorQuantizer` impls | Round-trip encode/decode correctness on fixtures. Recall@k smoke test on a canned dataset for each impl. TurboQuant recall@k must land within 2% of full-precision baseline. |
| `EntityExtractor` impls | Fixture-based unit tests (known text → known entities). Separate integration test loads the ONNX model and runs once. |
| `EntityDeduplicator` | Fixture tests including the "Apple / Apple Inc. / AAPL" case; asserts MENTIONS edges resolve to a single canonical entity. |
| `ArticleDeduplicator` | Fixtures for exact duplicates (hash) and near duplicates (slight text edits). Asserts `DUPLICATE_OF` edges are created and no vectors are written. |
| Migration | End-to-end integration test: seed a SQLite DB with representative data, run migrator, assert SurrealDB state matches (tables, tags normalization, federation records, edges). |
| Hybrid retrieval | Golden-set test: canned queries with expected top-k on a corpus pre-loaded with entities + edges. Regression guard for RRF weight tuning. |
| Graph queries | Hand-constructed entity graph tests 2-hop MENTIONS traversal correctness. |

New integration test files to add:
- `tests/migration_sqlite_to_surreal.rs`
- `tests/dedup_article.rs`
- `tests/dedup_entity.rs`
- `tests/quantizer_turboquant.rs` (depends on Track A crate)
- `tests/graph_expansion.rs`

## 12. Dependency changes

**Added:**
```toml
surrealdb = { version = "2", default-features = false, features = ["kv-surrealkv"] }
turboquant = { version = "0.1" }          # from Track A; path dep during parallel development
```

`sha2` is already a dep.

**Removed post-release:**
```toml
rusqlite  # kept through the migration window, removed after 1.0.0 ships
```

**Model artifacts (download-on-first-run, not bundled):**
- `dslim/bert-base-NER` ONNX weights (~420 MB).
- Existing `all-MiniLM-L6-v2` continues to be used for article embeddings and entity-dedup embedding.

## 13. Ship gates

Ordered list. All must hold before tagging 1.0.0.

1. `migrate` command works end-to-end on a representative 0.8 DB, verified by integration test.
2. All pre-existing hybrid-search tests pass against SurrealDB FTS.
3. Entity extraction works end-to-end; `MENTIONS` edges populated during ingestion.
4. Graph expansion in hybrid search returns plausible results (golden-set test passes).
5. Federation flows work against new `discovered_node` / `k2k_client` / edge model.
6. Binary size and startup time within 20% of 0.8 baseline (regression guard for SurrealDB dep weight).
7. Tray icon + CLI commands behave unchanged.
8. Full test suite green on macOS, Linux, and Windows.
9. `turboquant-rs` crate (Track A) published to crates.io or vendored at a tagged stable version.
10. `TurboQuantQuantizer` impl passes recall@k golden test within 2% of full-precision baseline.
11. `reindex --quantizer turboquant_v1 --store <id>` path exists and is tested end-to-end.
12. Article dedup (exact + near) creates `DUPLICATE_OF` edges without reindexing; covered by tests.
13. Entity dedup merges surface forms; `MENTIONS` edges resolve to canonical entity; "Apple / Apple Inc. / AAPL" test case passes.

## 14. Performance / concurrency

- SurrealDB embedded runs in-process, async. Tokio runtime handles it fine.
- LanceDB is also async. No runtime conflicts.
- Ingestion is serialized today and stays serialized; single-writer model of `kv-surrealkv` is not a regression.
- Startup: SurrealDB open is ~10–50 ms, comparable to SQLite. NER model load is ~500 ms (one-time, lazily on first extraction).
- Cold first-query latency with graph expansion: 2-hop traversal over ~10K entities is negligible next to vector search cost.

No dedicated performance budget work for 1.0.0. Post-migration measurement on real data informs `rrf_weights`, `graph_depth`, and dedup thresholds. These become tunable config.

## 15. Open questions

- **Entity-type taxonomy extensibility.** `entity.entity_type` is a `String`, not an enum, to avoid schema churn when adding GLiNER or custom types. A small curated set ships as the canonical list (`PERSON`, `ORG`, `LOCATION`, `MISC`); out-of-set types are accepted but flagged. Revisit when GLiNER lands.
- **Conversation→article grounding source.** `DISCUSSED` edges need a producer. Likely wired in the chat/retrieval layer (after a retrieval turn, attach the top-k article ids as `DISCUSSED` edges to the conversation message). Left for implementation plan to detail.
- **Graph RRF weights and dedup thresholds.** Defaults are documented (`rrf` = [1.0, 1.1, 0.7], entity-dedup = 0.85, article-near-dedup = 0.92). These are starting points; live tuning after shipping is expected.
- **LanceDB version versus `lance` crate patches.** The repo currently patches `lance` 0.19.2 in `patches/`. 1.0.0 stays on that pin; bumping LanceDB is out of scope for this spec.

## 16. Related specs (separate)

- **Track A:** `turboquant-rs` crate design — to be brainstormed separately. Covers algorithm implementation (PolarQuant + QJL), calibration for 384-dim MiniLM embeddings, validation on standard ANN benchmarks, crate API, and crates.io publication. 1.0.0 of knowledge-nexus-local depends on that crate being ready at gate 9.
