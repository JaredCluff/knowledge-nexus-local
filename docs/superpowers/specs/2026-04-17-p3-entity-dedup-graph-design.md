# P3: Entity Extraction, Dedup, and Knowledge Graph

## Overview

P3 adds three capabilities to the knowledge-nexus local agent:

1. **LLM-based entity extraction** — Extract structured entities (people, tools, topics, etc.) from article content using a local Ollama model.
2. **Content dedup with review queue** — Detect duplicate articles at ingest via content hash, skip silently, and queue for human review (reject or merge).
3. **Knowledge graph** — Store entities, tags, and relationships as SurrealDB graph edges. Compute article-to-article relatedness eagerly at ingest.

P3 builds on infrastructure from P1 (SurrealDB migration, `content_hash` field, `find_article_by_hash()`) and is a prerequisite for P4 (graph-powered tri-signal retrieval).

---

## Data Model

### New Tables

**`entity`** — Canonical entities extracted from articles.

| Field | Type | Notes |
|-------|------|-------|
| `id` | string | Deterministic: slugified `{type}:{name}` (e.g., `tool:rust`) |
| `name` | string | Display name ("Rust") |
| `entity_type` | string | One of: `topic`, `person`, `project`, `tool`, `concept`, `reference` |
| `description` | string (optional) | Short description from extraction context |
| `store_id` | string | Scoped to a knowledge store |
| `mention_count` | int | Denormalized count of MENTIONS edges |
| `created_at` | string | ISO 8601 |
| `updated_at` | string | ISO 8601 |

Index: `(store_id, name, entity_type)` unique — prevents duplicate entities within a store.

**`tag`** — Normalized tags, migrated from the article JSON `tags` array.

| Field | Type | Notes |
|-------|------|-------|
| `id` | string | Slugified tag name |
| `name` | string | Display name |
| `store_id` | string | Scoped to a knowledge store |
| `created_at` | string | ISO 8601 |

Index: `(store_id, name)` unique.

**`dedup_queue`** — Duplicate articles awaiting human review.

| Field | Type | Notes |
|-------|------|-------|
| `id` | string | Auto-generated |
| `store_id` | string | Which store the duplicate was detected in |
| `incoming_title` | string | Title of the incoming article |
| `incoming_content` | string | Full content of the incoming article |
| `incoming_source_type` | string | Source type of the incoming article |
| `incoming_source_id` | string (optional) | Source ID if available |
| `matched_article_id` | string | ID of the existing article that matched |
| `content_hash` | string | The hash that triggered the match |
| `status` | string | `pending`, `rejected`, `merged` |
| `created_at` | string | ISO 8601 |
| `resolved_at` | string (optional) | When the entry was resolved |

### New Graph Edges (SurrealDB RELATE)

**`TAGGED`** — Links an article to a tag.
- Direction: `article -> tag`
- Fields: `created_at` (string)

**`MENTIONS`** — Links an article to an entity it references.
- Direction: `article -> entity`
- Fields: `excerpt` (string — text snippet where entity appears), `confidence` (f32 — LLM confidence), `created_at` (string)

**`RELATED_TO`** — Links two articles that share entities.
- Direction: `article -> article`
- Fields: `shared_entity_count` (int), `strength` (f32 — normalized entity overlap), `created_at` (string), `updated_at` (string)
- Computed eagerly: when an article is ingested and entities extracted, find other articles sharing those entities and create/update edges.

### Modified Tables

**`article`** — Remove the `tags` field (JSON array). Tags are now stored as `tag` records with `TAGGED` edges.

---

## Entity Extraction Pipeline

### Ollama Integration

New module `src/knowledge/entity_extractor.rs` containing an `EntityExtractor` struct.

**Connection:** HTTP client to Ollama API at a configurable URL (default `http://localhost:11434`).

**Model:** Configurable, default `gemma4:e4b`.

**Prompt:** The system prompt constrains the model to return a JSON array of entities. Each entity has:

```json
{
  "name": "Rust",
  "entity_type": "tool",
  "description": "Systems programming language",
  "confidence": 0.95,
  "excerpt": "...written in Rust using async..."
}
```

Entity types are restricted to: `topic`, `person`, `project`, `tool`, `concept`, `reference`.

### Integration Point

Entity extraction runs inline during `ArticleService.embed_article()`, after chunking and embedding:

1. Call `EntityExtractor.extract()` with the full article text.
2. For each extracted entity: upsert into `entity` table (deduplicate by `store_id + name + entity_type`), increment `mention_count`.
3. Create `MENTIONS` edge from article to each entity with excerpt and confidence.
4. Query for other articles sharing the same entities. For each pair with shared entities, create or update a `RELATED_TO` edge with `shared_entity_count` and `strength`. Strength is computed as Jaccard similarity: `shared_entity_count / (entities_in_A + entities_in_B - shared_entity_count)`.

### Graceful Degradation

If Ollama is unreachable or returns an error:
- Log `tracing::warn!` with the error.
- Skip extraction entirely — the article is still stored and embedded.
- A `extract-entities` CLI command can backfill missed extractions later (identifies articles with no `MENTIONS` edges).

---

## Dedup Pipeline

### Detection

At the start of `ArticleService.create()`, before storing the article:

1. Compute `content_hash` via existing `src/store/hash.rs` (SHA-256 of normalized content).
2. Call `find_article_by_hash(store_id, content_hash)`.
3. If a match is found:
   - Log `tracing::info!` with the matched article ID.
   - Insert a `dedup_queue` entry with status `pending`, the incoming article's content, and the matched article ID.
   - Return the existing article's ID. Do not create a new article.
4. If no match: proceed with normal article creation.

### Review CLI

New subcommand `dedup-review`:

- `dedup-review list` — Show pending queue entries (title, matched article, date).
- `dedup-review reject <id>` — Set status to `rejected`. Discard the incoming content.
- `dedup-review merge <id>` — Replace the existing article's content with the incoming content. Preserve the article ID and all graph edges. Re-run embedding and entity extraction on the updated content. Set status to `merged`.

---

## Tag Migration

A one-time migration runs as part of the P3 schema upgrade in `src/store/migrations.rs`:

1. Bump schema version to `"1.0.0-p3"`.
2. Create the `tag` table and `TAGGED` edge schema.
3. For each article with a non-empty `tags` JSON array:
   - For each tag string: upsert a `tag` record (slugified ID, scoped to `store_id`).
   - Create a `TAGGED` edge from the article to the tag.
4. Remove the `tags` field from the `article` schema (`REMOVE FIELD tags ON article`).

Post-migration, tag operations use graph edges:

- **Add tag:** Upsert `tag` record, `RELATE article:id -> tagged -> tag:id`.
- **Remove tag:** `DELETE article:id->tagged WHERE out = tag:id`.
- **List tags for article:** `SELECT ->tagged->tag FROM article:id`.
- **List articles for tag:** `SELECT <-tagged<-article FROM tag:id`.

---

## Configuration

New `[extraction]` section in `config.toml`:

```toml
[extraction]
enabled = true
ollama_url = "http://localhost:11434"
model = "gemma4:e4b"
```

When `extraction.enabled = false`, the extraction step in `embed_article()` is skipped entirely (same as Ollama-down behavior, but intentional).

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `src/knowledge/entity_extractor.rs` | `EntityExtractor` struct, Ollama HTTP client, prompt construction, JSON response parsing |
| Modify | `src/knowledge/articles.rs` | Wire extraction into `embed_article()`, dedup check in `create()`, tag edge creation |
| Modify | `src/store/schema.rs` | `entity`, `tag`, `dedup_queue` DDL; `TAGGED`/`MENTIONS`/`RELATED_TO` edge schemas; bump to `1.0.0-p3` |
| Modify | `src/store/models.rs` | `Entity`, `Tag`, `DedupQueueEntry` structs |
| Modify | `src/store/mod.rs` | CRUD for entities, tags, dedup queue; edge creation/query; graph traversal helpers |
| Modify | `src/store/migrations.rs` | P3 migration: tag JSON to edges, schema version bump |
| Modify | `src/main.rs` | `dedup-review` and `extract-entities` CLI subcommands |
| Modify | `src/config.rs` | `[extraction]` config section with `ExtractionConfig` struct |
| Modify | `Cargo.toml` | Add `reqwest` for Ollama HTTP client |

**Not modified:** `src/vectordb/` (vector layer unchanged), `src/retrieval/` (graph-aware retrieval is P4).

---

## Testing Strategy

- **Unit tests:** Entity extractor JSON parsing (mock Ollama responses), dedup detection logic, tag slugification, entity ID generation, `RELATED_TO` strength calculation.
- **Integration tests:** Full ingest-with-extraction round-trip using a mock HTTP server for Ollama, dedup queue creation on duplicate insert, tag migration from JSON to edges, graph traversal queries.
- **Manual verification:** `extract-entities` backfill command, `dedup-review` CLI workflow.

---

## Out of Scope

- **Graph-aware retrieval** — P4 (tri-signal: vector + FTS + graph).
- **TurboQuant integration** — P5.
- **Entity merging/disambiguation** — If the LLM extracts "Rust" and "Rust language" as separate entities, they remain separate. Entity merging is a future enhancement.
- **Cross-store entity linking** — Entities are scoped to a single store. Cross-store graphs are a future consideration.
