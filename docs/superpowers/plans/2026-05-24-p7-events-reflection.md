# P7: Event Segmentation & Reflection — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to execute task-by-task.
>
> **Prerequisites:** P4-P6 merged or stacked. This plan branches off `feat/p6-spreading-activation`.

**Goal:** Add `Event` as a first-class memory node, segment conversations into events via LLM (with heuristic fallback), run scheduled reflection passes that consolidate clusters into summary memories using Nemori's Predict-Calibrate distillation. Reflections are themselves queryable memory.

**Architecture:** Event nodes live alongside Articles in SurrealDB, sharing the P5 multi-graph edge types. CompassMem's event taxonomy (causal/temporal/motivation/part-of) maps to P5 edges plus two new types. Segmentation is LLM-prompted (CompassMem Two-Step Alignment, mirrored from Nemori) with a silence-gap + topic-shift heuristic fallback. Reflection job runs on a scheduler (nightly cadence or N-ingests trigger), finds clusters by entity-overlap + temporal-proximity + P6 activation-density, applies Nemori Predict-Calibrate to produce minimal-information-delta summaries. Reflection confidence floor is `min(source_confidences)` — the compression-amplified-toxin defense from Lin et al.

**Tech Stack:** Existing — Rust, SurrealDB, Ollama (segmentation + reflection LLM calls), tokio for scheduler. No new substrates.

## Bibliography

- CompassMem (Hu et al., arXiv 2601.04726, Jan 2026) — event-graph and logic-map navigation
- Nemori (Ma et al., arXiv 2508.03341, Aug 2025) — Two-Step Alignment + Predict-Calibrate distillation
- Lin et al. (arXiv 2604.16548) — Compression-amplified-toxin defense
- Luo et al. (arXiv 2605.06716) — Storage → Reflection → Experience progression (P7 moves KNL from Storage to Reflection)

## File Structure

| Action | Path | Responsibility |
|---|---|---|
| Modify | `src/store/schema.rs` | `event`, `motivates`, `part_of`, `contains_evidence` tables; schema → `1.0.0-p7` |
| Modify | `src/store/models.rs` | `Event` struct + 3 new edge structs |
| Modify | `src/store/mod.rs` | Event CRUD + new edge CRUD |
| Modify | `src/store/migrations.rs` | P7 migration (DDL-only; no data backfill) |
| Create | `src/knowledge/events.rs` | LLM segmentation + heuristic fallback |
| Create | `src/knowledge/reflection.rs` | Cluster detection + Nemori Predict-Calibrate distillation |
| Create | `src/maintenance/mod.rs` | new top-level maintenance module |
| Create | `src/maintenance/scheduler.rs` | cron-style background runner |
| Modify | `src/knowledge/mod.rs` | Export new modules |
| Modify | `src/lib.rs` | Add `pub mod maintenance;` |
| Modify | `src/config/mod.rs` | `ReflectionConfig` section |
| Modify | `src/main.rs` | `reflect`, `segment-events` CLI subcommands |

---

### Task 1: Schema + models for Events

**Files:** `src/store/schema.rs`, `src/store/models.rs`

- [ ] **Step 1: TDD — add failing test for `Event` serde**

In `src/store/models.rs` `tests` module, add:

```rust
    #[test]
    fn test_event_serde_round_trip() {
        let e = Event {
            id: "e1".into(),
            store_id: "s1".into(),
            title: "AZ trip March 2026".into(),
            summary: "Family trip to Arizona mountains".into(),
            started_at: "2026-03-15T00:00:00Z".into(),
            ended_at: "2026-03-20T00:00:00Z".into(),
            participants: serde_json::json!(["alice", "bob"]),
            source_type: "conversation".into(),
            confidence: 0.85,
            extraction_method: ExtractionMethod::Llm,
            created_at: "2026-05-24T00:00:00Z".into(),
            updated_at: "2026-05-24T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        let d: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(d.title, "AZ trip March 2026");
        assert_eq!(d.extraction_method, ExtractionMethod::Llm);
    }

    #[test]
    fn test_contains_evidence_edge_serde_round_trip() {
        let edge = ContainsEvidenceEdge {
            from_event_id: "e1".into(),
            to_article_id: "a1".into(),
            confidence: 0.9,
            created_at: "2026-05-24T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&edge).unwrap();
        let d: ContainsEvidenceEdge = serde_json::from_str(&json).unwrap();
        assert_eq!(d.from_event_id, "e1");
    }
```

- [ ] **Step 2: Add `Event` struct and 3 edge structs**

In `src/store/models.rs`, after `ReferencesEdgeRow`:

```rust
/// Event: a first-class memory node representing a coherent time-bounded
/// experience (a conversation, a trip, an incident). P7.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub store_id: String,
    pub title: String,
    pub summary: String,
    pub started_at: String,
    pub ended_at: String,
    /// JSON array of participant names/ids.
    pub participants: serde_json::Value,
    /// "conversation" | "manual" | "derived"
    pub source_type: String,
    pub confidence: f64,
    pub extraction_method: ExtractionMethod,
    pub created_at: String,
    pub updated_at: String,
}

/// CONTAINS_EVIDENCE edge: event → article (evidence the event happened).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainsEvidenceEdge {
    pub from_event_id: String,
    pub to_article_id: String,
    pub confidence: f64,
    pub created_at: String,
}

/// MOTIVATES edge: event → event (one event motivated another).
/// CompassMem relation taxonomy. LLM-extracted only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MotivatesEdge {
    pub from_event_id: String,
    pub to_event_id: String,
    pub confidence: f64,
    pub rationale: Option<String>,
    pub extraction_method: ExtractionMethod,
    pub created_at: String,
}

/// PART_OF edge: event → event (hierarchical composition).
/// CompassMem relation taxonomy. LLM-extracted only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartOfEdge {
    pub from_event_id: String,
    pub to_parent_event_id: String,
    pub confidence: f64,
    pub extraction_method: ExtractionMethod,
    pub created_at: String,
}
```

- [ ] **Step 3: DDL additions**

In `src/store/schema.rs`, bump `SCHEMA_VERSION` to `"1.0.0-p7"`. At end of DDL string, append:

```sql
-- P7 event nodes and event-specific edges.

DEFINE TABLE IF NOT EXISTS event SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS store_id ON event TYPE string;
DEFINE FIELD IF NOT EXISTS title ON event TYPE string;
DEFINE FIELD IF NOT EXISTS summary ON event TYPE string;
DEFINE FIELD IF NOT EXISTS started_at ON event TYPE string;
DEFINE FIELD IF NOT EXISTS ended_at ON event TYPE string;
DEFINE FIELD IF NOT EXISTS participants ON event TYPE array DEFAULT [];
DEFINE FIELD IF NOT EXISTS participants.* ON event TYPE string;
DEFINE FIELD IF NOT EXISTS source_type ON event TYPE string DEFAULT "manual";
DEFINE FIELD IF NOT EXISTS confidence ON event TYPE float DEFAULT 1.0;
DEFINE FIELD IF NOT EXISTS extraction_method ON event TYPE string DEFAULT "user_asserted";
DEFINE FIELD IF NOT EXISTS created_at ON event TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at ON event TYPE string;
DEFINE INDEX IF NOT EXISTS event_store_idx ON event FIELDS store_id;
DEFINE INDEX IF NOT EXISTS event_time_idx ON event FIELDS started_at;

-- CONTAINS_EVIDENCE: event → article
DEFINE TABLE IF NOT EXISTS contains_evidence TYPE RELATION IN event OUT article SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS confidence ON contains_evidence TYPE float DEFAULT 1.0;
DEFINE FIELD IF NOT EXISTS created_at ON contains_evidence TYPE string;
DEFINE INDEX IF NOT EXISTS contains_evidence_unique ON contains_evidence FIELDS in, out UNIQUE;

-- MOTIVATES: event → event
DEFINE TABLE IF NOT EXISTS motivates TYPE RELATION IN event OUT event SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS confidence ON motivates TYPE float;
DEFINE FIELD IF NOT EXISTS rationale ON motivates TYPE option<string>;
DEFINE FIELD IF NOT EXISTS extraction_method ON motivates TYPE string DEFAULT "llm";
DEFINE FIELD IF NOT EXISTS created_at ON motivates TYPE string;
DEFINE INDEX IF NOT EXISTS motivates_unique ON motivates FIELDS in, out UNIQUE;

-- PART_OF: child event → parent event
DEFINE TABLE IF NOT EXISTS part_of TYPE RELATION IN event OUT event SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS confidence ON part_of TYPE float;
DEFINE FIELD IF NOT EXISTS extraction_method ON part_of TYPE string DEFAULT "llm";
DEFINE FIELD IF NOT EXISTS created_at ON part_of TYPE string;
DEFINE INDEX IF NOT EXISTS part_of_unique ON part_of FIELDS in, out UNIQUE;
```

- [ ] **Step 4: Run tests + commit**

```bash
cargo test --lib store::models 2>&1 | tail -10
cargo test --lib store 2>&1 | tail -10
git add src/store/schema.rs src/store/models.rs
git commit -m "feat(p7): Event node + ContainsEvidence/Motivates/PartOf edges"
```

---

### Task 2: P7 migration (DDL-only)

**Files:** `src/store/migrations.rs`

- [ ] **Step 1: Add migration call to `run_migrations`**

In `src/store/migrations.rs`, after the P5 migration block (which handles p1/p2/p3 → p5), add:

```rust
    // P7 migration: DDL-only (event table + new edges). Existing data
    // unaffected; events table starts empty, populated by Task 4 segmenter.
    if current_version.starts_with("1.0.0-p1")
        || current_version.starts_with("1.0.0-p2")
        || current_version.starts_with("1.0.0-p3")
        || current_version.starts_with("1.0.0-p5")
    {
        tracing::info!("P7 migration: schema upgrade only (no data changes)");
        // DDL is already applied via run_ddl above; nothing to do beyond
        // bumping the version. The schema additions in P7 Task 1 land via
        // the next `cargo run` and are idempotent.
    }
```

- [ ] **Step 2: Add migration test**

```rust
    #[tokio::test]
    async fn migration_p5_to_p7_idempotent_dll_only() {
        let db = setup_p5_corpus().await; // helper to seed p5-state version
        run_migrations(&db).await.expect("first run");
        run_migrations(&db).await.expect("second run idempotent");

        // Event table should exist and be empty
        let mut resp = db.query("SELECT count() AS n FROM event GROUP ALL")
            .await.expect("count event").check().expect("check");
        #[derive(serde::Deserialize)] struct Cnt { n: i64 }
        let cnts: Vec<Cnt> = resp.take(0).unwrap_or_default();
        assert_eq!(cnts.first().map(|c| c.n).unwrap_or(0), 0);

        // Version is now p7
        let mut resp = db.query(
            "SELECT version FROM _schema_version WHERE id = type::thing('_schema_version', 'current')"
        ).await.unwrap().check().unwrap();
        #[derive(serde::Deserialize)] struct V { version: String }
        let vs: Vec<V> = resp.take(0).unwrap_or_default();
        assert_eq!(vs.first().map(|v| v.version.as_str()), Some("1.0.0-p7"));
    }
```

(You'll need a `setup_p5_corpus()` helper similar to the existing `setup_p3_corpus()`. Just seeds an article + sets version=`1.0.0-p5`.)

- [ ] **Step 3: Test + commit**

```bash
cargo test --lib store::migrations 2>&1 | tail -10
git add src/store/migrations.rs
git commit -m "feat(p7): migration scaffolding for event-table schema upgrade"
```

---

### Task 3: Event CRUD + edge helpers

**Files:** `src/store/mod.rs`

- [ ] Add Store trait methods:
  - `create_event(&Event) -> Result<()>`
  - `get_event(event_id) -> Result<Option<Event>>`
  - `list_events_for_store(store_id) -> Result<Vec<Event>>`
  - `create_contains_evidence_edge(event_id, article_id, confidence)`
  - `create_motivates_edge(from_event, to_event, confidence, rationale)`
  - `create_part_of_edge(child_event, parent_event, confidence)`
  - `list_events_for_article(article_id) -> Result<Vec<Event>>` (via CONTAINS_EVIDENCE reverse)

- [ ] SurrealStore impls mirror existing patterns (RELATE for edges, CONTENT for nodes, swallow UNIQUE conflicts).

- [ ] 6 tests covering each method.

```bash
git commit -m "feat(p7): Event + event-edge CRUD on Store trait"
```

---

### Task 4: Event segmentation (`src/knowledge/events.rs`)

**Files:** `src/knowledge/events.rs`, `src/knowledge/mod.rs`

- [ ] Create `EventSegmenter` mirroring `EntityExtractor` HTTP pattern (Ollama).
- [ ] Prompt extracts `{title, summary, started_at, ended_at, participants, evidence_spans}` from a conversation/article batch.
- [ ] Heuristic fallback: silence-gap + topic-shift signals from entity overlap (when LLM unavailable). Produces coarser segments with same schema.
- [ ] 5 unit tests on prompt construction + JSON parsing.

```bash
git commit -m "feat(p7): LLM event segmentation with heuristic fallback"
```

---

### Task 5: Predict-Calibrate reflection (`src/knowledge/reflection.rs`)

**Files:** `src/knowledge/reflection.rs`, `src/knowledge/mod.rs`

- [ ] Create `Reflector` struct.
- [ ] Three-step Predict-Calibrate (Nemori):
  1. **Predict:** LLM, given current memory state + cluster intent, predicts what the cluster contains.
  2. **Compare:** LLM, given actual cluster contents, extracts what's IN the cluster but NOT in the prediction.
  3. **Store:** prediction-error delta becomes the reflection content.
- [ ] Cluster detection: entity-overlap + temporal-proximity + P6 activation-density (call into `ActivationEngine`).
- [ ] Reflection stored as `Article` with `source_type="reflection"`, `extraction_method=Llm`, and a `reflects: Vec<article_id>` field.
- [ ] **Compression-amplified-toxin defense:** reflection confidence = `min(source_confidences)`, never max.
- [ ] 4 tests including the toxin-defense (high-confidence reflection from low-confidence sources gets capped low).

```bash
git commit -m "feat(p7): Predict-Calibrate reflection (Nemori) with toxin defense"
```

---

### Task 6: Article `reflects` field + source_type='reflection'

**Files:** `src/store/schema.rs`, `src/store/models.rs`, `src/knowledge/articles.rs`

- [ ] Add nullable `reflects: array<string>` to article schema.
- [ ] Add `reflects: Vec<String>` (default empty) to `Article` struct.
- [ ] Update `articles.rs` so reflection-typed articles propagate the `reflects` array.
- [ ] Add `Store::list_reflections_for_article(article_id)` helper that finds reflections pointing to this article via `reflects`.
- [ ] 2 tests: serde round-trip; list-reflections lookup.

```bash
git commit -m "feat(p7): article.reflects field for reflection→source linkage"
```

---

### Task 7: Maintenance scheduler

**Files:** `src/maintenance/mod.rs`, `src/maintenance/scheduler.rs`, `src/lib.rs`

- [ ] Create `MaintenanceScheduler` with `register(job_name, cron_spec, handler_fn)`.
- [ ] Default: register `reflection_job` (nightly 03:00 local) and `segmentation_job` (on-demand only).
- [ ] Use tokio + `cron` crate (or simple interval timer) — keep dep light.
- [ ] **Safe restart:** scheduler tracks last-run timestamp in `_maintenance_runs` table; on startup, schedules next run accordingly.
- [ ] **Idempotency keys:** each job records its idempotency key (e.g., `reflection:store_id:date`); duplicate-day runs are no-ops.
- [ ] 3 tests.

```bash
git commit -m "feat(p7): MaintenanceScheduler for reflection + segmentation cadence"
```

---

### Task 8: Wire reflection into ingest path (light)

**Files:** `src/knowledge/articles.rs`

- [ ] After article ingest completes (post-embed), increment a rate-based counter; when counter crosses threshold (config: `ingests_per_reflection_trigger`, default 100), submit a reflection job to scheduler.
- [ ] Per-store counters.
- [ ] Test: ingest threshold triggers scheduler.

```bash
git commit -m "feat(p7): rate-triggered reflection on ingest threshold"
```

---

### Task 9: CLI surface

**Files:** `src/main.rs`

- [ ] `reflect --store <id> [--dry-run]` — on-demand reflection
- [ ] `segment-events --since <date> [--store <id>]` — on-demand segmentation over conversations
- [ ] `event list [--store] [--since] [--until]` — list events
- [ ] Each: open store via `open_store_or_bail`, build scheduler/segmenter/reflector, run.

```bash
git commit -m "feat(p7): CLI reflect + segment-events + event list"
```

---

### Task 10: E2E test

**Files:** `src/knowledge/reflection.rs`

- [ ] Integration test: seed 5 articles sharing entities → run reflection → verify 1 Reflection-typed article emitted with `reflects` populated, confidence ≤ min(source_confidences).

```bash
git commit -m "test(p7): end-to-end reflection over 5-article cluster"
```

---

### Task 11: Push + open PR

- [ ] Final build/test/clippy, ~155-160 tests.
- [ ] Push branch.
- [ ] Open PR with base `feat/p6-spreading-activation`.

```bash
git push -u origin feat/p7-events-reflection
gh pr create --base feat/p6-spreading-activation --title "P7: Event Segmentation & Reflection" --body "..."
```

---

## Self-Review Checklist

- Schema bumped to 1.0.0-p7
- Event node + 3 new edges
- LLM segmentation with heuristic fallback (graceful degradation)
- Nemori Predict-Calibrate distillation (3-step prompt chain)
- Compression-amplified-toxin defense (confidence floor, not ceiling)
- MaintenanceScheduler with idempotency keys
- All P7 features opt-in by config; no breakage of existing P3-P6 paths
- All tests pass; no new clippy warnings on P7 code
