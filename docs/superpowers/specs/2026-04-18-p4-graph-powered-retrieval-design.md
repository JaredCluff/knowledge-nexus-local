# P4: Graph-Powered Tri-Signal Retrieval

## Goal

Integrate the P3 knowledge graph (entities, MENTIONS, RELATED_TO, TAGGED edges) into the search pipeline as a third retrieval signal alongside vector similarity and full-text search. The graph should transparently improve result quality — users never interact with it directly during search.

## Architecture

The current retrieval pipeline:

```
Query -> Vector Search -> FTS Search -> RRF Merge -> Rerank -> Expand -> Results
```

Becomes:

```
Query -> Vector Search  -+
      -> FTS Search     -+-> Tri-Signal RRF Merge -> Rerank -> Expand -> Results
      -> Graph Search   -+
```

A new `GraphSearcher` component sits alongside the existing vector and FTS paths. It identifies entities matching the query, traverses graph edges to find connected articles, and produces a ranked list that feeds into an N-way RRF merge. Adaptive weighting ensures the graph signal scales with entity coverage — when no entities match, results are pure vector+FTS with no dilution.

The CLI `search` command is unified onto the same `LocalRouter::route()` pipeline the API uses, so every search entry point gets tri-signal fusion.

A lightweight `graph` CLI command provides read-only inspection of entities and edges for troubleshooting.

## Components

### 1. GraphSearcher (`src/retrieval/graph.rs`)

Responsible for entity matching, graph traversal, and scoring.

**Entity matching:**
1. Tokenize the query, filter stop words (reuse `QueryExpander`'s stop word list).
2. For each meaningful term and multi-word combination, query the entity table:
   - Exact match: `entity.name = "Rust"` -> `tool:rust`
   - Prefix match: `entity.name STARTS WITH "async"` -> `concept:async-runtime`
3. This is a SurrealDB query, not an LLM call. Fast and deterministic.

**Graph traversal:**
1. **Direct mentions**: For each matched entity, follow MENTIONS edges backward to articles. Score = `mentions.confidence * entity.mention_count_normalized`.
2. **One-hop related**: For those articles, follow RELATED_TO edges to neighbors. Score = `related_to.strength * 0.5` (decay factor prevents distant articles from dominating).
3. Deduplicate articles. Sum scores for articles reached via multiple paths.

**Scoring function:**

```
graph_score = sum(entity_confidence * path_weight)
```

Where `path_weight` = 1.0 for direct MENTIONS, 0.5 for one-hop RELATED_TO.

**Output:** A ranked `Vec<K2KResult>` with `ResultProvenance { store_type: "graph" }`.

### 2. Tri-Signal RRF Merge (refactored `src/retrieval/hybrid.rs`)

The existing `merge_hybrid()` is refactored into a general `merge_signals()` that accepts N ranked lists with weights.

**Adaptive weight calculation:**

```
entity_coverage = matched_entities.len() / meaningful_query_terms.len()
graph_weight = config.graph_weight_max * entity_coverage
```

With defaults: `vector_weight = 1.0`, `keyword_weight = 1.1`, `graph_weight_max = 1.0`.

When `entity_coverage = 0`, graph contributes nothing. When `entity_coverage = 1.0`, graph is a full peer signal.

**Merge formula per article:**

```
rrf_score = (vector_weight / (K + vector_rank))
          + (keyword_weight / (K + keyword_rank))
          + (graph_weight / (K + graph_rank))
```

Where `K = 60.0` (existing RRF constant). Articles appearing in only one or two lists get scores from those lists only — no penalty for absence.

### 3. CLI Unification (`src/main.rs`)

The `search` command stops calling `search::search_files()` directly and calls `LocalRouter::route()` instead. This gives every CLI search the full hybrid+graph pipeline.

**CLI flags:**
- `search <query>` — full pipeline, default behavior
- `--limit N` — max results (default 10, already exists)
- `--store <id>` — restrict to a specific store
- `--verbose` — show provenance info (which signals contributed, RRF scores)

**Default output:**

```
Found 5 results:

1. [0.87] Building async services with Tokio
   "Tokio provides an async runtime for Rust applications..."

2. [0.74] Rust ownership fundamentals
   "The borrow checker ensures memory safety..."
```

**Verbose output** adds a `via:` line per result:

```
1. [0.87] Building async services with Tokio
   "Tokio provides an async runtime for Rust applications..."
   via: vector + graph(tool:rust, tool:tokio)
```

### 4. Graph Debug Command (`src/main.rs`)

A lightweight CLI subcommand for inspecting graph data. Read-only.

**`nexus graph entity "Rust"`** — show entity details, mentioning articles, co-mentioned entities:

```
Entity: Rust (tool)
  Mentions: 15 articles
  Description: "Systems programming language"

  Top articles:
    1. Building async services with Tokio (confidence: 0.95)
    2. Rust ownership fundamentals (confidence: 0.92)

  Related entities (co-mentioned):
    tool:tokio (12 shared articles)
    concept:borrow-checking (8 shared articles)
```

**`nexus graph article <id>`** — show entities, related articles, tags for an article:

```
Article: Building async services with Tokio

  Entities mentioned:
    tool:rust (confidence: 0.95)
    tool:tokio (confidence: 0.92)

  Related articles (via RELATED_TO):
    Rust ownership fundamentals (strength: 0.8, shared: 2 entities)

  Tags: rust, async, web-services
```

**`nexus graph stats`** — aggregate graph statistics:

```
Store: My Knowledge Base
  Entities: 47 (12 tool, 10 topic, 8 concept, 7 person, 6 project, 4 reference)
  Articles with extractions: 42/50
  MENTIONS edges: 156
  RELATED_TO edges: 89
  Avg entities per article: 3.7
```

### 5. Configuration (`src/config/mod.rs`)

New `[retrieval]` section in `config.toml`:

```toml
[retrieval]
rrf_k = 60.0
vector_weight = 1.0
keyword_weight = 1.1
graph_weight_max = 1.0
graph_hops = 1
```

Implemented as a `RetrievalConfig` struct with a `Default` impl providing these values. If the section is absent from config, defaults apply. Zero migration burden for existing users.

### 6. New Store Trait Methods

Added to the `Store` trait in `src/store/mod.rs`:

- `search_entities_by_name(store_id: &str, terms: &[&str]) -> Result<Vec<Entity>>` — exact and prefix entity lookup for GraphSearcher.
- `list_articles_for_entities(entity_ids: &[&str]) -> Result<Vec<(Article, f64)>>` — batch lookup with MENTIONS confidence scores. Avoids N+1 queries.
- `count_entities_by_type(store_id: &str) -> Result<HashMap<String, usize>>` — for `graph stats`.
- `list_co_mentioned_entities(entity_id: &str) -> Result<Vec<(Entity, usize)>>` — entities frequently appearing alongside the given entity, with shared article count. For `graph entity`.

## Error Handling

- Entity matching returns empty: graph weight goes to 0, search works normally via vector+FTS.
- Graph traversal errors: log warning, return empty graph results, vector+FTS carry the search. Same graceful degradation pattern as entity extraction (Ollama down -> skip, continue).
- No articles have entity extractions: graph signal is empty everywhere, effectively pure vector+FTS until `extract-entities` backfill runs.

## File Structure

| File | Purpose |
|------|---------|
| `src/retrieval/graph.rs` | `GraphSearcher` — entity matching, traversal, scoring |
| `src/retrieval/hybrid.rs` | Refactor `merge_hybrid` -> `merge_signals` (N-way RRF) |
| `src/config/mod.rs` | Add `RetrievalConfig` struct |
| `src/store/mod.rs` | New trait methods for graph queries |
| `src/main.rs` | Unify CLI `search`, add `graph` subcommand |

No new crates or external dependencies. All graph queries use existing SurrealDB infrastructure.

## Testing Strategy

**Unit tests (no DB):**
- `merge_signals()` with mock ranked lists — verify RRF math, weight scaling, dedup across N signals
- Adaptive weight calculation — verify coverage ratio produces correct weights at boundaries (0, 0.5, 1.0)
- Graph scoring — verify path decay, multi-path score aggregation

**Integration tests (embedded SurrealDB):**
- `GraphSearcher` end-to-end: seed articles + entities + edges, run graph search, verify results and ranking
- `search_entities_by_name` — exact match, prefix match, no match, multiple matches
- Tri-signal merge with real data — verify graph results appear in final ranking at correct positions
- Zero entity coverage — verify graceful fallback to pure vector+FTS with no score dilution

**CLI test:**
- Verify `search` command routes through `LocalRouter` (not `search_files`)

## Design Principles

- **Graph is infrastructure, not interface.** Users see better results. They do not see entities or edges during normal search.
- **Adaptive, not aggressive.** Graph signal self-adjusts. Rich entity coverage = strong graph influence. No entity matches = zero graph influence. No dilution.
- **Sensible defaults.** A user who never touches `[retrieval]` config gets optimal behavior out of the box. Config knobs exist for tuning, not for initial setup.
- **Graceful degradation.** Missing entities, failed traversals, empty graph — search always works, falling back to vector+FTS.
