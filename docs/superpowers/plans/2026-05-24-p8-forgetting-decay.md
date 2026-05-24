# P8: Forgetting, Decay, and Compaction — Implementation Plan

> **REQUIRED SUB-SKILL:** superpowers:subagent-driven-development
>
> **Prerequisites:** P4-P7 merged or stacked. Branches off `feat/p7-events-reflection`.

**Goal:** Add salience-based tier transitions (Hot/Warm/Cold/Archive), access tracking, P8 decay function as a P6 activation input, compaction-via-reflection for redundant low-salience clusters, append-only audit log, pin/unpin, soft-archive only. The Animus principle — "quarantine, not delete" — is the governing constraint.

**Architecture in one paragraph.** Every article and event grows P8 metadata: `access_count`, `last_accessed_at`, `importance_score`, `tier`, `pinned`. The salience function (SYNAPSE-aligned default: `salience = importance · exp(-λ · days_since_access)`) drives tier transitions on a nightly job. Tier feeds the P6 activation engine via a per-node multiplier applied during PPR seed scoring. Compaction finds Cold-tier redundant clusters (entity-overlap + activation-density) and creates P7-style reflections covering them; originals are NOT deleted — they're tier-Archive with a `compacted_into: reflection_id` backlink. All transitions go through an append-only `_audit_log`. Pin overrides everything: pinned items never tier-down, never compact.

**Tech Stack:** Existing — Rust, SurrealDB, tokio scheduler (P7 Task 7). Per-edge-type decay in PPR uses existing P6 `ActivationConfig`. No new substrates.

## Bibliography

- SYNAPSE (arXiv 2601.02744) — node decay ablation (#1 contributor, removing it drops Temporal F1 50.1→14.2)
- MemoryOS (arXiv 2506.06326) — heat formula as alternative salience approach
- Generative Agents (Park et al., named in Du survey 2603.07670 §9.4) — recency × relevance × importance
- MemoryBank — Ebbinghaus-curve decay
- Lin et al. (arXiv 2604.16548) — Forget/Rollback governance phase, audit-mandatory tier separation

## File Structure

| Action | Path | Responsibility |
|---|---|---|
| Modify | `src/store/schema.rs` | Add tier/decay fields to article + event; new `_audit_log` table; schema → `1.0.0-p8` |
| Modify | `src/store/models.rs` | `Tier` enum; extend `Article` + `Event` with P8 fields; `AuditLogEntry` |
| Modify | `src/store/mod.rs` | Tier helpers, access tracking, pin/unpin, audit-log CRUD |
| Modify | `src/store/migrations.rs` | P5/P7 → P8 migration: backfill access_count=0, tier=Hot, importance from entity-degree |
| Create | `src/maintenance/decay.rs` | Salience function (4 implementations); tier transition logic |
| Create | `src/maintenance/audit.rs` | Append-only audit log writer + query |
| Create | `src/maintenance/compaction.rs` | Cluster-redundancy detection → P7 reflection; mark sources Archive |
| Modify | `src/maintenance/mod.rs` | Register decay + compaction job specs |
| Modify | `src/retrieval/reranker.rs` | Tier-aware salience weighting |
| Modify | `src/router/executor.rs` | Record access on every retrieval hit |
| Modify | `src/retrieval/activation.rs` | Pass salience multiplier into PPR personalization vector |
| Modify | `src/config/mod.rs` | `DecayConfig` section (lambda, tier thresholds, salience formula choice) |
| Modify | `src/main.rs` | `pin`, `unpin`, `decay-status`, `compact`, `audit-log` CLI |

---

### Task 1: Tier enum + P8 model fields + DDL

**Files:** `src/store/models.rs`, `src/store/schema.rs`

- [ ] **Step 1: Add `Tier` enum to models.rs**

After `ExtractionMethod` (P5 Task 1), add:

```rust
/// Memory tier for salience-based retention (P8).
/// Tier transitions are nightly; pinned items never tier down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Recent or frequently-accessed; retrieval gets full salience.
    Hot,
    /// Idle for moderate time; retrieval gets ~0.5× salience.
    Warm,
    /// Mostly idle; retrieval gets ~0.1× salience and excluded from default queries.
    Cold,
    /// Quarantined; only surfaced with explicit `include_archive=true`.
    Archive,
}
```

- [ ] **Step 2: Add P8 fields to `Article` struct**

Add after `reflects: Vec<String>`:

```rust
    /// Number of times this article was returned by a recall query (P8).
    #[serde(default)]
    pub access_count: i64,
    /// RFC3339 of last access (P8). Empty string = never accessed.
    #[serde(default)]
    pub last_accessed_at: String,
    /// Importance score in [0.0, 1.0]. Computed from entity-degree + manual
    /// boosts. Default 0.5 for unpinned user articles (P8).
    #[serde(default = "default_importance")]
    pub importance_score: f64,
    /// Current salience tier (P8).
    #[serde(default = "default_tier")]
    pub tier: Tier,
    /// User-pinned items never tier-down or compact (P8).
    #[serde(default)]
    pub pinned: bool,
    /// If this article was compacted into a reflection, this is the
    /// reflection's article id. P8 compaction. Quarantine, never delete.
    #[serde(default)]
    pub compacted_into: Option<String>,
```

Add module-level default functions:

```rust
fn default_importance() -> f64 { 0.5 }
fn default_tier() -> Tier { Tier::Hot }
```

- [ ] **Step 3: Add same fields to `Event` struct** (parallel; events tier alongside articles).

- [ ] **Step 4: Add `AuditLogEntry` struct**

```rust
/// Append-only audit log entry (P8).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub id: String,
    pub store_id: String,
    /// One of: tier_change, pin, unpin, compact, hard_delete (admin only),
    /// access_recorded (rate-limited), retrieval_event.
    pub action: String,
    /// "article" | "event" | "reflection"
    pub subject_type: String,
    /// id of the affected entity
    pub subject_id: String,
    /// Free-form details, e.g. {"from_tier": "Hot", "to_tier": "Warm", "reason": "nightly decay"}
    pub details: serde_json::Value,
    pub recorded_at: String,
}
```

- [ ] **Step 5: DDL additions in schema.rs**

Bump `SCHEMA_VERSION` to `"1.0.0-p8"`. Add P8 fields to existing `article` and `event` DDL blocks:

```sql
-- P8 fields (added via IF NOT EXISTS to upgrade in place)
DEFINE FIELD IF NOT EXISTS access_count ON article TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS last_accessed_at ON article TYPE string DEFAULT "";
DEFINE FIELD IF NOT EXISTS importance_score ON article TYPE float DEFAULT 0.5;
DEFINE FIELD IF NOT EXISTS tier ON article TYPE string DEFAULT "hot";
DEFINE FIELD IF NOT EXISTS pinned ON article TYPE bool DEFAULT false;
DEFINE FIELD IF NOT EXISTS compacted_into ON article TYPE option<string>;

-- Same for event
DEFINE FIELD IF NOT EXISTS access_count ON event TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS last_accessed_at ON event TYPE string DEFAULT "";
DEFINE FIELD IF NOT EXISTS importance_score ON event TYPE float DEFAULT 0.5;
DEFINE FIELD IF NOT EXISTS tier ON event TYPE string DEFAULT "hot";
DEFINE FIELD IF NOT EXISTS pinned ON event TYPE bool DEFAULT false;
DEFINE FIELD IF NOT EXISTS compacted_into ON event TYPE option<string>;

-- P8 audit log (append-only)
DEFINE TABLE IF NOT EXISTS _audit_log SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS store_id ON _audit_log TYPE string;
DEFINE FIELD IF NOT EXISTS action ON _audit_log TYPE string;
DEFINE FIELD IF NOT EXISTS subject_type ON _audit_log TYPE string;
DEFINE FIELD IF NOT EXISTS subject_id ON _audit_log TYPE string;
DEFINE FIELD IF NOT EXISTS details ON _audit_log TYPE object DEFAULT {};
DEFINE FIELD IF NOT EXISTS recorded_at ON _audit_log TYPE string;
DEFINE INDEX IF NOT EXISTS _audit_log_subject_idx ON _audit_log FIELDS subject_id;
DEFINE INDEX IF NOT EXISTS _audit_log_time_idx ON _audit_log FIELDS recorded_at;
DEFINE INDEX IF NOT EXISTS _audit_log_store_idx ON _audit_log FIELDS store_id;
```

- [ ] **Step 6: Test + commit**

Add 3 serde tests (Tier round-trip, Article with P8 fields, AuditLogEntry). Update all `Article {` and `Event {` literal constructions in src/ to include the new fields with defaults.

```bash
git commit -m "feat(p8): Tier enum + P8 metadata fields + audit log schema"
```

---

### Task 2: P7 → P8 migration

**Files:** `src/store/migrations.rs`

- [ ] Backfill existing articles: `access_count = 0`, `last_accessed_at = created_at` (treat creation as initial access), `importance_score = 0.5` (or derived from entity-degree if entities exist), `tier = "hot"`, `pinned = false`.
- [ ] Same for events.
- [ ] 2 tests: forward migration sets fields correctly; idempotency.

```bash
git commit -m "feat(p8): P7 → P8 migration backfilling tier metadata"
```

---

### Task 3: Store trait — access tracking + tier + pin/unpin

**Files:** `src/store/mod.rs`

- [ ] `record_article_access(article_id) -> Result<()>` — increments counter + updates last_accessed_at + records audit entry
- [ ] `set_article_tier(article_id, new_tier, reason) -> Result<()>` — audit-logged
- [ ] `pin_article(article_id) -> Result<()>` / `unpin_article(article_id) -> Result<()>`
- [ ] Equivalent for events
- [ ] `list_articles_by_tier(store_id, tier) -> Result<Vec<Article>>` — admin/debugging
- [ ] `write_audit_log(entry: &AuditLogEntry) -> Result<()>` (append-only)
- [ ] `list_audit_log(store_id, since, limit) -> Result<Vec<AuditLogEntry>>`
- [ ] 8 tests.

```bash
git commit -m "feat(p8): tier + access + pin/unpin + audit log on Store trait"
```

---

### Task 4: `DecayConfig` + salience function

**Files:** `src/config/mod.rs`, `src/maintenance/decay.rs`

- [ ] `DecayConfig` with: `lambda` (default 0.02 ≈ 35-day half-life), tier thresholds (Hot ≥ 0.5, Warm ≥ 0.1, Cold ≥ 0.01, Archive < 0.01), `salience_formula` enum ("activation_driven" default, "memory_os_heat", "generative_agents", "ebbinghaus").
- [ ] `salience(importance, last_accessed_at, formula, now) -> f64` — main entry point dispatching by formula.
- [ ] 4 implementations:
  - **SYNAPSE-aligned activation-driven (default):** `salience = importance · exp(-lambda · days_since_access)`. Lambda=0.02 ≈ 35-day half-life.
  - **MemoryOS heat:** `heat = α·visits + β·interaction_length + γ·recency_factor` with `μ=1e7` decay constant.
  - **Generative Agents:** `salience = w_rec · exp(-decay · days) + w_rel · relevance + w_imp · importance`. Per-component weights configurable.
  - **MemoryBank Ebbinghaus:** `salience = exp(-t/S)` with reinforcement-on-access raising S.
- [ ] 8 tests across the formulas (2 each: typical case + edge case).

```bash
git commit -m "feat(p8): DecayConfig + 4 salience formulas (SYNAPSE default)"
```

---

### Task 5: Tier transition job

**Files:** `src/maintenance/decay.rs`

- [ ] `nightly_tier_transition(store_id, db, config) -> Result<TransitionReport>`:
  1. For each article + event in the store, compute current salience via `decay_config.formula`.
  2. Determine new tier from salience + thresholds.
  3. If new_tier != current_tier AND `!pinned`: call `set_article_tier` (audit-logged).
  4. Pinned items skipped silently.
  5. Return counts: `{hot_to_warm: N, warm_to_cold: M, cold_to_archive: K, promotions: ...}`.
- [ ] **Access promotes to Hot.** Implementation: `record_article_access` always sets tier=Hot if not pinned + currently lower; audit-logged with reason `"access_promote"`.
- [ ] 4 tests.

```bash
git commit -m "feat(p8): nightly tier transition + access-promote-to-Hot"
```

---

### Task 6: Tier-aware reranker + activation salience input

**Files:** `src/retrieval/reranker.rs`, `src/retrieval/activation.rs`

- [ ] Reranker (existing P4): multiply final confidence by a tier-aware factor (Hot=1.0, Warm=0.5, Cold=0.1, Archive=0.0 unless `include_archive`).
- [ ] ActivationEngine: in personalization vector construction (Task 6 of P6), multiply seed weight by `salience(article)`. SYNAPSE's "decay drives PPR" tile is finally complete.
- [ ] Add `include_archive: bool` to RetrievalConfig (default false).
- [ ] 3 tests: tier=Hot vs Warm produces different ranking; Archive excluded by default; Archive surfaces with override.

```bash
git commit -m "feat(p8): tier-aware reranker + salience-weighted PPR seeds

Wires P8 tier metadata into both the P4 reranker AND the P6 PPR
personalization vector. The latter completes SYNAPSE's reported #1
ablation contributor: removing node decay drops Temporal F1 50.1→14.2."
```

---

### Task 7: Compaction module

**Files:** `src/maintenance/compaction.rs`

- [ ] `compact_low_salience(store_id, db, reflector, dry_run) -> Result<CompactionReport>`:
  1. Find articles with `tier == Cold` AND `!pinned`.
  2. Group by entity-overlap clusters (use P3 entity_overlap edges).
  3. For each cluster of size ≥ `min_compact_cluster_size` (default 5):
     - Build a `ReflectionCluster` from cluster sources.
     - Call P7 `Reflector::reflect()`.
     - If delta non-empty: store reflection article; for each source, set `compacted_into = reflection_id` + tier = Archive.
  4. Pinned articles always excluded.
  5. **Dry-run mode** prints what would happen; no writes.
- [ ] Audit-logged.
- [ ] 3 tests including dry-run.

```bash
git commit -m "feat(p8): compaction-via-reflection for redundant Cold clusters

Cold-tier articles in redundant clusters (shared entity) are compacted:
P7 Reflector synthesizes a single reflection covering all of them; the
originals are marked compacted_into=reflection_id and tier=Archive.

Quarantine principle: no source is deleted; explicit query for compacted
sources surfaces them. Pinned articles are always excluded."
```

---

### Task 8: Record retrieval access in pipeline

**Files:** `src/router/executor.rs`

- [ ] After `merge_signals` returns final K2KResults, fire-and-forget `record_article_access` for each returned article id.
- [ ] Use a background task spawn so it doesn't block the response.
- [ ] **Rate limiting:** only record one access per article per minute (use the existing `_audit_log` to check). Otherwise a noisy user-loop floods the log.
- [ ] 1 test: querying for an article increments its access_count.

```bash
git commit -m "feat(p8): pipeline records access on retrieval hits (rate-limited)"
```

---

### Task 9: Register nightly job + CLI

**Files:** `src/maintenance/mod.rs`, `src/main.rs`

- [ ] Register `decay_nightly` and `compaction_weekly` in `MaintenanceScheduler` with idempotency keys `decay:{store_id}:{date}` and `compact:{store_id}:{date_week}`.
- [ ] CLI:
  - `pin <article_id>` / `unpin <article_id>`
  - `decay-status [--store]` — print salience histogram + tier counts
  - `compact [--store] [--dry-run]` — explicit compaction trigger
  - `audit-log [--store] [--since] [--limit]` — browse audit log
  - `tier <article_id> <tier>` — admin override (audit-logged)

```bash
git commit -m "feat(p8): nightly job registrations + pin/unpin/decay-status/compact/audit-log CLI"
```

---

### Task 10: E2E test

**Files:** `src/store/mod.rs`

- [ ] Seed 10 articles with varied `last_accessed_at` timestamps.
- [ ] Run nightly_tier_transition.
- [ ] Verify: recent articles → Hot; mid-aged → Warm; old → Cold.
- [ ] Pinned articles stay Hot even if old.
- [ ] Audit log contains transition records.

```bash
git commit -m "test(p8): end-to-end tier transition + pin override + audit trail"
```

---

### Task 11: Push + open PR (base = `feat/p7-events-reflection`)

```bash
git push -u origin feat/p8-forgetting-decay
gh pr create --base feat/p7-events-reflection --title "P8: Forgetting, Decay, Compaction" --body "..."
```

---

## Self-Review Checklist

- Schema → 1.0.0-p8
- Tier enum + P8 fields on Article + Event
- Salience function (4 implementations; SYNAPSE-aligned default)
- Tier transitions audit-logged
- Pin overrides everything
- Compaction never deletes (quarantine)
- Access promotes to Hot
- PPR personalization scaled by salience (closes SYNAPSE's #1 ablation gap)
- All tests pass; no new clippy warnings
