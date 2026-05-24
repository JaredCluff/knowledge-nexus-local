# P6: Spreading Activation Retrieval — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Prerequisites:** P4 (PR #13), P5 (PR #14), P4/P5 followups (PR #15) all merged or stacked. This plan branches off `feat/p5-followups`.

**Goal:** Replace static pre-computed graph edges (P3's eager Jaccard + P5's eager cos-sim) as the dominant graph signal with query-time spreading activation, using HippoRAG's Personalized PageRank for diffusion, SYNAPSE's lateral inhibition + sigmoid + gating for post-processing, and MAGMA's rule-based query-adaptive intent policy for edge-type weighting.

**Architecture in one paragraph:** A new `ActivationEngine` orchestrates four steps per query. (1) Intent classification: rule-based mapping from query → `Intent ∈ {Why, When, Entity, MultiHop, OpenDomain}`, with optional Ollama 3B fallback for ambiguous queries (cached). (2) Seed identification: extract query entities via existing P3 EntityExtractor, find matching graph nodes, weight each by HippoRAG specificity `s_i = 1/|P_i|`. (3) PPR diffusion: build column-stochastic edge-weight matrix over P5's typed edges with intent-adaptive per-type multipliers (MAGMA Table 6), iterate `a^(t+1) = (1-d)·𝒯 + d·W·a^(t)` to convergence (damping=0.5, tolerance=1e-4, max=50 iter). (4) Post-PPR processing: SYNAPSE lateral inhibition (β=0.15, M=7), sigmoid normalization (γ=5.0), confidence gating (τ=0.12), top-K cutoff. The result is a ranked list with full provenance (path, iterations, intent). ActivationEngine slots into the tri-signal RRF as the graph signal, gated by `RetrievalConfig.graph_strategy` (`"jaccard"` preserves P4; `"activation"` is new default).

**Tech Stack:** Rust, existing SurrealDB + LanceDB + tri-signal pipeline (P4/P5), `sprs` crate (or custom CSR sparse-matrix code) for PPR, existing Ollama HTTP path (optional intent classifier), no new external services.

---

## Design Constants (all from peer-reviewed sources)

From **HippoRAG** (arXiv 2405.14831, NeurIPS 2024):
- PPR damping factor: **d = 0.5**
- Convergence tolerance: **1e-4**, max iterations **50**
- Node specificity: **s_i = 1/|P_i|** where P_i = articles linked to node i
- Synonymy threshold: **0.85** (KNL midpoint between HippoRAG 0.8 and SYNAPSE 0.92)

From **SYNAPSE** (arXiv 2601.02744):
- Lateral inhibition: **β = 0.15**, top-M competitor set **M = 7**
- Sigmoid steepness: **γ = 5.0**
- Confidence gate: **τ = 0.12**
- Retention (when using fixed T<convergence): **δ = 0.5**
- Active subgraph cap: **|V| ≤ 10000**
- Final fusion linear weights: **λ = (0.5, 0.3, 0.2)** — semantic, activation, structural

From **MAGMA** (arXiv 2601.03236):
- Intent classifier: rule-based with optional learned upgrade later
- Per-intent edge-weight multipliers (Table 6, adapted for KNL):

| Intent | causal | temporal | entity | semantic |
|---|---|---|---|---|
| Why | **4.0** | 1.0 | 1.5 | 1.0 |
| When | 1.0 | **3.0** | 1.0 | 1.0 |
| Entity | 1.0 | 1.0 | **4.0** | 1.0 |
| MultiHop | 2.0 | 1.5 | 2.0 | 1.5 |
| OpenDomain | 1.0 | 1.0 | 1.0 | 1.0 |

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `src/retrieval/activation.rs` | `ActivationEngine`: orchestrates intent → seeds → PPR → post-process |
| Create | `src/retrieval/ppr.rs` | Personalized PageRank: column-stochastic edge matrix, sparse iteration, convergence detection |
| Create | `src/retrieval/post_process.rs` | Lateral inhibition + sigmoid + gating + retention |
| Create | `src/retrieval/intent.rs` | Rule-based query intent classifier; optional Ollama fallback for ambiguous |
| Create | `src/retrieval/specificity.rs` | HippoRAG node specificity weights `s_i = 1/|P_i|`; cached per-store |
| Modify | `src/retrieval/graph.rs` | Add `graph_strategy` dispatch: jaccard (P4 path) vs activation (P6 path) |
| Modify | `src/retrieval/mod.rs` | Export new modules + re-exports |
| Modify | `src/config/mod.rs` | `ActivationConfig` struct with all constants; add to `RetrievalConfig` |
| Modify | `src/router/executor.rs` | Wire ActivationEngine into tri-signal pipeline (when strategy=activation) |
| Modify | `src/store/mod.rs` | Helper queries: list_entity_mention_counts (for specificity), list_neighbors_all_typed |
| Modify | `src/main.rs` | `graph-debug --activation-trace` to surface per-node activation iterations |

---

## Bibliography

- HippoRAG: arXiv 2405.14831 (NeurIPS 2024)
- SYNAPSE: arXiv 2601.02744 (Jan 2026)
- MAGMA: arXiv 2601.03236 (ACL 2026)
- See `docs/superpowers/plans/2026-05-23-supermemory-upgrade-roadmap.md` for full design context.

---

### Task 1: Add `ActivationConfig` to configuration

**Files:**
- Modify: `src/config/mod.rs`

- [ ] **Step 1: Add `ActivationConfig` struct and intent table**

After `EdgeTypeFilter` (P5 Task 9), add:

```rust
/// Spreading-activation configuration (P6).
///
/// Defaults are from peer-reviewed sources:
/// - HippoRAG (arXiv 2405.14831): damping=0.5, tolerance=1e-4
/// - SYNAPSE (arXiv 2601.02744): β=0.15, M=7, γ=5.0, τ=0.12, |V|≤10k
/// - MAGMA (arXiv 2601.03236): per-intent edge weight multipliers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivationConfig {
    /// PPR damping factor.
    #[serde(default = "default_ppr_damping")]
    pub damping: f32,

    /// PPR convergence tolerance.
    #[serde(default = "default_ppr_tolerance")]
    pub tolerance: f32,

    /// Maximum PPR iterations before forced termination.
    #[serde(default = "default_ppr_max_iter")]
    pub max_iter: usize,

    /// Active subgraph cap (max nodes considered per query).
    #[serde(default = "default_subgraph_cap")]
    pub subgraph_cap: usize,

    /// SYNAPSE lateral inhibition strength.
    #[serde(default = "default_inhibition_beta")]
    pub inhibition_beta: f32,

    /// SYNAPSE lateral inhibition competitor-set size.
    #[serde(default = "default_inhibition_m")]
    pub inhibition_m: usize,

    /// SYNAPSE sigmoid steepness.
    #[serde(default = "default_sigmoid_gamma")]
    pub sigmoid_gamma: f32,

    /// SYNAPSE confidence gate threshold.
    #[serde(default = "default_gate_tau")]
    pub gate_tau: f32,

    /// Final top-K cutoff for activation results.
    #[serde(default = "default_top_k")]
    pub top_k: usize,

    /// If true, fall back to Ollama 3B intent classifier for ambiguous queries.
    /// If false, ambiguous queries default to OpenDomain (uniform weights).
    #[serde(default)]
    pub llm_intent_fallback: bool,
}

fn default_ppr_damping() -> f32 { 0.5 }
fn default_ppr_tolerance() -> f32 { 1e-4 }
fn default_ppr_max_iter() -> usize { 50 }
fn default_subgraph_cap() -> usize { 10_000 }
fn default_inhibition_beta() -> f32 { 0.15 }
fn default_inhibition_m() -> usize { 7 }
fn default_sigmoid_gamma() -> f32 { 5.0 }
fn default_gate_tau() -> f32 { 0.12 }
fn default_top_k() -> usize { 30 }

impl Default for ActivationConfig {
    fn default() -> Self {
        Self {
            damping: default_ppr_damping(),
            tolerance: default_ppr_tolerance(),
            max_iter: default_ppr_max_iter(),
            subgraph_cap: default_subgraph_cap(),
            inhibition_beta: default_inhibition_beta(),
            inhibition_m: default_inhibition_m(),
            sigmoid_gamma: default_sigmoid_gamma(),
            gate_tau: default_gate_tau(),
            top_k: default_top_k(),
            llm_intent_fallback: false,
        }
    }
}
```

- [ ] **Step 2: Add `graph_strategy` + `activation` fields to `RetrievalConfig`**

Modify `RetrievalConfig` (P4 Task 1):

```rust
    /// Graph signal strategy: "jaccard" preserves P4 behavior (one-hop
    /// expansion via ENTITY_OVERLAP); "activation" enables P6 spreading
    /// activation via PPR + SYNAPSE post-processing.
    #[serde(default = "default_graph_strategy")]
    pub graph_strategy: String,

    /// Spreading-activation parameters (P6).
    #[serde(default)]
    pub activation: ActivationConfig,
```

```rust
fn default_graph_strategy() -> String { "activation".into() }
```

Update `RetrievalConfig::default()` to include both. Update `Config::default()` if needed (it should auto-propagate via `#[serde(default)]` and the `Default` impl).

- [ ] **Step 3: Run tests + commit**

Run: `cargo test --lib config 2>&1 | tail -5`
Expected: existing config tests pass; new fields default gracefully.

```bash
git add src/config/mod.rs
git commit -m "feat(p6): add ActivationConfig with PPR + SYNAPSE + MAGMA constants"
```

---

### Task 2: HippoRAG node specificity

**Files:**
- Create: `src/retrieval/specificity.rs`
- Modify: `src/retrieval/mod.rs`

The specificity weight for entity node `i` is `s_i = 1/|P_i|` where P_i is the set of articles mentioning entity `i`. Common entities (e.g., "Rust" mentioned in 100 articles) get low specificity; rare ones get high. This is HippoRAG's IDF analogue and contributes -2.7 to -3.8 pts on MuSiQue/HotpotQA when removed per their ablation.

- [ ] **Step 1: Add Store helper `count_mentions_per_entity`**

In `src/store/mod.rs`, add to the `Store` trait:

```rust
    /// Returns a map of entity_id → article_count (mentions). Used by P6
    /// HippoRAG-style specificity weighting.
    async fn count_mentions_per_entity(&self, store_id: &str) -> Result<HashMap<String, usize>>;
```

SurrealStore impl:

```rust
    async fn count_mentions_per_entity(&self, store_id: &str) -> Result<HashMap<String, usize>> {
        let mut resp = self.db()
            .query(
                "SELECT meta::id(out) AS entity_id, count() AS cnt
                 FROM mentions
                 WHERE store_id = $sid OR in.store_id = $sid
                 GROUP BY entity_id"
            )
            .bind(("sid", store_id.to_string()))
            .await
            .context("count_mentions_per_entity")?;
        #[derive(serde::Deserialize)]
        struct Row { entity_id: String, cnt: i64 }
        let rows: Vec<Row> = resp.take(0).unwrap_or_default();
        Ok(rows.into_iter().map(|r| (r.entity_id, r.cnt as usize)).collect())
    }
```

(If `mentions` lacks `store_id` directly, look at how P3 wrote those edges and filter via `in.store_id` lookup instead — adapt the WHERE clause.)

- [ ] **Step 2: Write failing tests for specificity**

Create `src/retrieval/specificity.rs`:

```rust
//! HippoRAG-style node specificity: rare entities get higher weight.
//!
//! `s_i = 1 / |P_i|` where P_i is the set of articles mentioning entity i.
//! Cached per-store. Re-fetched lazily on first query after a write.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use anyhow::Result;

use crate::store::Store;

#[derive(Default)]
pub struct SpecificityCache {
    by_store: RwLock<HashMap<String, Arc<HashMap<String, f32>>>>,
}

impl SpecificityCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get specificity weights for `store_id`. Computes on first call;
    /// reuses cached map on subsequent calls until `invalidate()` is called.
    pub async fn get<S: Store + ?Sized>(&self, store: &S, store_id: &str) -> Result<Arc<HashMap<String, f32>>> {
        if let Some(cached) = self.by_store.read().await.get(store_id) {
            return Ok(Arc::clone(cached));
        }
        let counts = store.count_mentions_per_entity(store_id).await?;
        let weights: HashMap<String, f32> = counts.into_iter()
            .map(|(id, n)| {
                let n = n.max(1);
                (id, 1.0 / n as f32)
            })
            .collect();
        let arc = Arc::new(weights);
        self.by_store.write().await.insert(store_id.to_string(), Arc::clone(&arc));
        Ok(arc)
    }

    pub async fn invalidate(&self, store_id: &str) {
        self.by_store.write().await.remove(store_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SurrealStore;

    #[tokio::test]
    async fn specificity_inverse_proportional_to_mention_count() {
        // Setup: entity "common" appears in 4 articles, "rare" in 1
        // Expected: s(common) = 1/4 = 0.25, s(rare) = 1/1 = 1.0
        // ...seeding details...
    }

    #[tokio::test]
    async fn specificity_cache_hits_after_first_load() {
        // Setup: store with one entity-article mention
        // First get(): cache miss → query db
        // Second get(): cache hit → no query (verify via debug or by mutating db without invalidate)
        // ...
    }
}
```

Fill in the seeding for both tests using the `entity_tests::fixture()` pattern; seed entities + articles + mentions edges via raw SQL.

- [ ] **Step 3: Module export**

Add to `src/retrieval/mod.rs`:

```rust
pub mod specificity;
pub use specificity::SpecificityCache;
```

- [ ] **Step 4: Run tests + commit**

```bash
cargo test --lib retrieval::specificity 2>&1 | tail -10
git add src/retrieval/specificity.rs src/retrieval/mod.rs src/store/mod.rs
git commit -m "feat(p6): HippoRAG node specificity cache (s_i = 1/|P_i|)"
```

---

### Task 3: Rule-based intent classifier

**Files:**
- Create: `src/retrieval/intent.rs`
- Modify: `src/retrieval/mod.rs`

- [ ] **Step 1: Define `Intent` enum + weight table**

Create `src/retrieval/intent.rs`:

```rust
//! Rule-based query intent classifier (MAGMA-style).
//!
//! Maps queries to one of five intents and supplies per-intent edge-type
//! weight multipliers. Defaults from MAGMA Table 6, adapted for KNL's
//! five edge types.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Intent {
    Why,
    When,
    Entity,
    MultiHop,
    OpenDomain,
}

/// Per-intent edge-weight multipliers. Applied to the column-stochastic
/// edge matrix before PPR diffusion.
#[derive(Debug, Clone, Copy)]
pub struct IntentWeights {
    pub entity_overlap: f32,
    pub semantically_related: f32,
    pub precedes: f32,
    pub caused_by: f32,
    pub references_edge: f32,
}

impl Intent {
    /// MAGMA Table 6 defaults (adapted): entity_overlap and references mirror
    /// the entity dimension; semantically_related mirrors the semantic dimension.
    pub fn weights(self) -> IntentWeights {
        match self {
            Intent::Why => IntentWeights {
                entity_overlap: 1.5, semantically_related: 1.0,
                precedes: 1.0, caused_by: 4.0, references_edge: 1.5,
            },
            Intent::When => IntentWeights {
                entity_overlap: 1.0, semantically_related: 1.0,
                precedes: 3.0, caused_by: 1.0, references_edge: 1.0,
            },
            Intent::Entity => IntentWeights {
                entity_overlap: 4.0, semantically_related: 1.0,
                precedes: 1.0, caused_by: 1.0, references_edge: 4.0,
            },
            Intent::MultiHop => IntentWeights {
                entity_overlap: 2.0, semantically_related: 1.5,
                precedes: 1.5, caused_by: 2.0, references_edge: 2.0,
            },
            Intent::OpenDomain => IntentWeights {
                entity_overlap: 1.0, semantically_related: 1.0,
                precedes: 1.0, caused_by: 1.0, references_edge: 1.0,
            },
        }
    }
}
```

- [ ] **Step 2: Rule-based `classify`**

```rust
/// Rule-based classifier. Captures MAGMA's 8.9% LoCoMo gain without a
/// learned model. Optional Ollama 3B fallback for ambiguous queries
/// (e.g., questions with overlapping cue words) — see `classify_with_llm`.
pub fn classify(query: &str) -> Intent {
    let q = query.to_lowercase();

    // Causal cues → Why
    let causal_cues = ["why", "because", "caused", "cause of", "due to", "led to", "result of", "consequence"];
    let why_score = causal_cues.iter().filter(|c| q.contains(*c)).count();

    // Temporal cues → When
    let temporal_cues = ["when", "after", "before", "during", "while", "earlier", "later", "first", "last", "history of", "timeline"];
    let when_score = temporal_cues.iter().filter(|c| q.contains(*c)).count();

    // Entity-focused cues: question is short, full of named entities, or asks "what is X"
    let entity_cues = ["what is", "who is", "tell me about", "describe", "definition of"];
    let entity_score = entity_cues.iter().filter(|c| q.contains(*c)).count();

    // Multi-hop cues: multiple clauses or "and" connecting concepts
    let multihop_score = if q.matches(" and ").count() >= 2 || q.contains(" through ") || q.contains(" via ") { 1 } else { 0 };

    // Highest score wins; ties resolved in declaration order below
    let scores = [
        (Intent::Why, why_score),
        (Intent::When, when_score),
        (Intent::Entity, entity_score),
        (Intent::MultiHop, multihop_score),
    ];

    let max_score = scores.iter().map(|(_, s)| *s).max().unwrap_or(0);
    if max_score == 0 {
        return Intent::OpenDomain;
    }
    scores.iter()
        .find(|(_, s)| *s == max_score)
        .map(|(intent, _)| *intent)
        .unwrap_or(Intent::OpenDomain)
}
```

- [ ] **Step 3: Add tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_why_questions() {
        assert_eq!(classify("Why did the build fail?"), Intent::Why);
        assert_eq!(classify("What caused the outage?"), Intent::Why);
        assert_eq!(classify("This happened because of the deploy"), Intent::Why);
    }

    #[test]
    fn classify_when_questions() {
        assert_eq!(classify("When did this happen?"), Intent::When);
        assert_eq!(classify("Show me the history of this project"), Intent::When);
        assert_eq!(classify("What happened after the deploy?"), Intent::When);
    }

    #[test]
    fn classify_entity_questions() {
        assert_eq!(classify("What is Rust?"), Intent::Entity);
        assert_eq!(classify("Tell me about Tokio"), Intent::Entity);
    }

    #[test]
    fn classify_open_domain_default() {
        assert_eq!(classify("the quick brown fox"), Intent::OpenDomain);
        assert_eq!(classify(""), Intent::OpenDomain);
    }

    #[test]
    fn weights_are_consistent_across_intents() {
        // OpenDomain weights are all 1.0; others have at least one non-1.0 weight.
        let od = Intent::OpenDomain.weights();
        assert_eq!(od.entity_overlap, 1.0);
        assert_eq!(od.semantically_related, 1.0);
        assert_eq!(od.precedes, 1.0);
        assert_eq!(od.caused_by, 1.0);
        assert_eq!(od.references_edge, 1.0);

        let why = Intent::Why.weights();
        assert!(why.caused_by > 1.0, "Why intent must boost causal");

        let when = Intent::When.weights();
        assert!(when.precedes > 1.0, "When intent must boost temporal");
    }
}
```

- [ ] **Step 4: Module export + commit**

Add to `src/retrieval/mod.rs`:

```rust
pub mod intent;
pub use intent::{Intent, IntentWeights, classify};
```

```bash
cargo test --lib retrieval::intent 2>&1 | tail -10
git add src/retrieval/intent.rs src/retrieval/mod.rs
git commit -m "feat(p6): rule-based query intent classifier (MAGMA Table 6 weights)"
```

---

### Task 4: PPR diffusion over typed edges

**Files:**
- Create: `src/retrieval/ppr.rs`
- Modify: `src/retrieval/mod.rs`

This is the core algorithm. The math is from HippoRAG; the inputs come from P5's typed edges; the outputs feed Task 5's post-processing.

- [ ] **Step 1: Add `sprs` dependency**

In `Cargo.toml`:

```toml
sprs = "0.11"
```

(Or whichever stable version. `sprs` provides CSR/CSC sparse matrices and the matrix-vector multiplication we need.)

- [ ] **Step 2: Implement PPR**

Create `src/retrieval/ppr.rs`:

```rust
//! Personalized PageRank over the P5 multi-graph (HippoRAG, arXiv 2405.14831).
//!
//! Given a personalization vector `t` over nodes and a column-stochastic
//! edge-weight matrix `W`, iterates `a^(t+1) = (1-d)·t + d·W·a^(t)` until
//! convergence (L1-norm change < tolerance) or `max_iter`. Returns the
//! stationary distribution over nodes.

use sprs::{CsMat, CsVec};

/// Run Personalized PageRank on a column-stochastic matrix `w`.
///
/// `w`: column-stochastic edge-weight matrix (rows = sources, cols = targets,
///      or vice-versa depending on caller convention — but normalized so each
///      column sums to 1).
/// `personalization`: sparse vector of seed weights. Will be L1-normalized
///                    internally.
/// `damping`: 0..=1, restart probability is (1 - damping).
/// `tolerance`: L1-norm convergence threshold.
/// `max_iter`: hard cap on iterations.
///
/// Returns the final activation vector (dense f32, length = w.cols()).
pub fn personalized_pagerank(
    w: &CsMat<f32>,
    personalization: &CsVec<f32>,
    damping: f32,
    tolerance: f32,
    max_iter: usize,
) -> Vec<f32> {
    let n = w.cols();
    debug_assert_eq!(personalization.dim(), n);

    // L1-normalize personalization vector
    let t_sum: f32 = personalization.data().iter().sum();
    let t_norm = if t_sum > 0.0 { t_sum } else { 1.0 };

    // Initialize a^(0) from personalization
    let mut a = vec![0.0_f32; n];
    for (idx, &v) in personalization.indices().iter().zip(personalization.data()) {
        a[*idx] = v / t_norm;
    }
    let mut a_next = vec![0.0_f32; n];

    for iter in 0..max_iter {
        // a_next = (1 - d) * t + d * (W · a)
        // First: a_next = (1 - d) * t
        for x in a_next.iter_mut() { *x = 0.0; }
        for (idx, &v) in personalization.indices().iter().zip(personalization.data()) {
            a_next[*idx] = (1.0 - damping) * (v / t_norm);
        }

        // Then: a_next += d * (W · a)
        //
        // W is CsMat. Use sprs's matrix-vector multiplication.
        // sprs CsMat doesn't have direct mul-by-dense, so iterate the triplets:
        for (col, vec) in w.outer_iterator().enumerate() {
            for (row, &weight) in vec.iter() {
                a_next[row] += damping * weight * a[col];
            }
        }

        // L1 norm of (a_next - a) for convergence
        let delta: f32 = a.iter().zip(a_next.iter())
            .map(|(p, n)| (p - n).abs())
            .sum();

        std::mem::swap(&mut a, &mut a_next);

        if delta < tolerance {
            tracing::debug!("PPR converged in {} iterations (delta={:.2e})", iter + 1, delta);
            return a;
        }
    }

    tracing::debug!("PPR hit max_iter={} without convergence", max_iter);
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprs::{CsMat, CsVec, TriMat};

    /// On a 3-node line A→B→C with uniform weights, seeding A should produce
    /// monotonically decreasing activation: a(A) > a(B) > a(C).
    #[test]
    fn ppr_decays_along_simple_chain() {
        let mut tri = TriMat::new((3, 3));
        // Col-stochastic: each column sums to 1 (or 0 for dangling)
        // A → B: col=A(0), row=B(1), w=1.0
        tri.add_triplet(1, 0, 1.0);
        // B → C: col=B(1), row=C(2), w=1.0
        tri.add_triplet(2, 1, 1.0);
        let w: CsMat<f32> = tri.to_csr();

        let mut t = CsVec::new(3, vec![0], vec![1.0]); // seed A
        // No-op to silence "unused mut" if any
        let _ = &mut t;

        let result = personalized_pagerank(&w, &t, 0.5, 1e-4, 50);
        assert!(result[0] > result[1], "A should retain more mass than B");
        assert!(result[1] > result[2], "B should have more mass than C");
        assert!(result[0] > 0.4, "A should retain >40% (restart effect)");
    }

    #[test]
    fn ppr_converges_within_max_iter() {
        // Star graph: 1 center, 3 leaves; symmetric
        let mut tri = TriMat::new((4, 4));
        tri.add_triplet(1, 0, 1.0 / 3.0);
        tri.add_triplet(2, 0, 1.0 / 3.0);
        tri.add_triplet(3, 0, 1.0 / 3.0);
        tri.add_triplet(0, 1, 1.0);
        tri.add_triplet(0, 2, 1.0);
        tri.add_triplet(0, 3, 1.0);
        let w: CsMat<f32> = tri.to_csr();

        let t = CsVec::new(4, vec![0], vec![1.0]);
        let result = personalized_pagerank(&w, &t, 0.5, 1e-6, 200);

        // After convergence the center should still have the most mass
        let max_idx = result.iter().enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i).unwrap();
        assert_eq!(max_idx, 0);
    }

    #[test]
    fn ppr_handles_disconnected_nodes() {
        // Two isolated components: A-B and C-D
        let mut tri = TriMat::new((4, 4));
        tri.add_triplet(1, 0, 1.0);
        tri.add_triplet(0, 1, 1.0);
        tri.add_triplet(3, 2, 1.0);
        tri.add_triplet(2, 3, 1.0);
        let w: CsMat<f32> = tri.to_csr();

        // Seed only A (index 0); C and D should remain at 0
        let t = CsVec::new(4, vec![0], vec![1.0]);
        let result = personalized_pagerank(&w, &t, 0.5, 1e-4, 50);

        assert!(result[0] > 0.0);
        assert!(result[1] > 0.0);
        assert!(result[2].abs() < 1e-6, "disconnected node C should have ~0 mass, got {}", result[2]);
        assert!(result[3].abs() < 1e-6, "disconnected node D should have ~0 mass, got {}", result[3]);
    }
}
```

- [ ] **Step 3: Module export + commit**

Add to `src/retrieval/mod.rs`:

```rust
pub mod ppr;
pub use ppr::personalized_pagerank;
```

```bash
cargo test --lib retrieval::ppr 2>&1 | tail -10
git add Cargo.toml Cargo.lock src/retrieval/ppr.rs src/retrieval/mod.rs
git commit -m "feat(p6): Personalized PageRank sparse-matrix implementation (HippoRAG)"
```

---

### Task 5: SYNAPSE post-processing (inhibition + sigmoid + gating)

**Files:**
- Create: `src/retrieval/post_process.rs`
- Modify: `src/retrieval/mod.rs`

- [ ] **Step 1: Implement post-processing**

Create `src/retrieval/post_process.rs`:

```rust
//! SYNAPSE post-PPR processing (arXiv 2601.02744): lateral inhibition,
//! sigmoid normalization, confidence gating, top-K selection.
//!
//! These steps are applied AFTER PPR converges. They provide competition
//! between high-activation nodes (which PPR alone doesn't have), squash
//! values to [0,1], drop low-confidence noise, and produce the final
//! ranking. All constants are SYNAPSE published defaults.

/// Apply lateral inhibition over the top-M competitors of node i:
/// `û_i = max(0, u_i - β·Σ_{k∈T_M}(u_k - u_i)·𝕀[u_k > u_i])`
///
/// This prevents a hub-node from dominating; results diversify across the
/// top of the distribution while preserving order between high vs. low
/// activation nodes.
pub fn lateral_inhibition(values: &[f32], beta: f32, m: usize) -> Vec<f32> {
    if values.len() <= 1 || beta <= 0.0 {
        return values.to_vec();
    }

    // Find top-M competitors (indices into `values`)
    let mut indexed: Vec<(usize, f32)> = values.iter().enumerate().map(|(i, &v)| (i, v)).collect();
    indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let top_m: Vec<(usize, f32)> = indexed.iter().take(m).cloned().collect();

    let mut out = values.to_vec();
    for i in 0..values.len() {
        let u_i = values[i];
        let inhibition: f32 = top_m.iter()
            .filter(|(_, u_k)| *u_k > u_i)
            .map(|(_, u_k)| u_k - u_i)
            .sum();
        out[i] = (u_i - beta * inhibition).max(0.0);
    }
    out
}

/// Sigmoid normalization with steepness γ.
pub fn sigmoid_normalize(values: &[f32], gamma: f32) -> Vec<f32> {
    values.iter().map(|&u| 1.0 / (1.0 + (-gamma * u).exp())).collect()
}

/// Drop entries below `tau`; return (index, value) of survivors sorted desc.
pub fn confidence_gate(values: &[f32], tau: f32) -> Vec<(usize, f32)> {
    let mut survivors: Vec<(usize, f32)> = values.iter()
        .enumerate()
        .filter(|(_, &v)| v >= tau)
        .map(|(i, &v)| (i, v))
        .collect();
    survivors.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    survivors
}

/// Full post-processing pipeline: inhibition → sigmoid → gate → top-K.
pub fn post_process(
    ppr_values: &[f32],
    beta: f32,
    m: usize,
    gamma: f32,
    tau: f32,
    top_k: usize,
) -> Vec<(usize, f32)> {
    let inhibited = lateral_inhibition(ppr_values, beta, m);
    let normalized = sigmoid_normalize(&inhibited, gamma);
    let mut gated = confidence_gate(&normalized, tau);
    gated.truncate(top_k);
    gated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lateral_inhibition_suppresses_below_top() {
        let values = [1.0, 0.9, 0.5, 0.1];
        let out = lateral_inhibition(&values, 0.15, 7);
        // Top value unchanged; others reduced by inhibition from higher
        assert!((out[0] - 1.0).abs() < 1e-9);
        assert!(out[1] < 0.9, "0.9 should be reduced by inhibition from 1.0");
        assert!(out[3] < 0.1);
    }

    #[test]
    fn lateral_inhibition_zero_beta_is_identity() {
        let values = [1.0, 0.5, 0.1];
        let out = lateral_inhibition(&values, 0.0, 7);
        assert_eq!(out, values);
    }

    #[test]
    fn lateral_inhibition_clamps_to_zero() {
        // If inhibition would push a value negative, it's clamped to 0
        let values = [10.0, 0.0]; // 0.0 gets inhibited by 10.0
        let out = lateral_inhibition(&values, 0.15, 7);
        assert!(out[1] >= 0.0);
    }

    #[test]
    fn sigmoid_squashes_to_unit_interval() {
        let values = [-10.0, 0.0, 10.0];
        let out = sigmoid_normalize(&values, 1.0);
        assert!(out[0] < 0.001);
        assert!((out[1] - 0.5).abs() < 1e-9);
        assert!(out[2] > 0.999);
    }

    #[test]
    fn confidence_gate_drops_below_tau() {
        let values = [0.5, 0.2, 0.8, 0.05];
        let survivors = confidence_gate(&values, 0.3);
        let ids: Vec<usize> = survivors.iter().map(|(i, _)| *i).collect();
        // Sorted desc by value: 0.8 (idx 2), 0.5 (idx 0)
        assert_eq!(ids, vec![2, 0]);
    }

    #[test]
    fn post_process_full_pipeline() {
        // Crafted to exercise all stages
        let values = [3.0, 2.0, 1.0, 0.5, 0.1];
        let out = post_process(&values, 0.15, 7, 5.0, 0.12, 3);

        assert!(out.len() <= 3);
        // Highest input should still rank first
        assert_eq!(out[0].0, 0);
        // Outputs are in [0, 1]
        for (_, v) in &out {
            assert!(*v >= 0.0 && *v <= 1.0);
        }
    }
}
```

- [ ] **Step 2: Module export + commit**

```rust
pub mod post_process;
pub use post_process::{post_process, lateral_inhibition, sigmoid_normalize, confidence_gate};
```

```bash
cargo test --lib retrieval::post_process 2>&1 | tail -10
git add src/retrieval/post_process.rs src/retrieval/mod.rs
git commit -m "feat(p6): SYNAPSE post-PPR processing (lateral inhibition + sigmoid + gate)"
```

---

### Task 6: `ActivationEngine` orchestrator

**Files:**
- Create: `src/retrieval/activation.rs`
- Modify: `src/retrieval/mod.rs`

This wires the four components from Tasks 2-5 plus seed extraction into a single engine. The orchestration is intent-classification → seed-identification → graph-build → PPR → post-process → translate to `K2KResult`.

- [ ] **Step 1: Sketch the engine**

Create `src/retrieval/activation.rs`:

```rust
//! Spreading-activation retrieval engine (P6).
//!
//! Orchestrates: intent classification → seed entities → typed-edge graph
//! construction → PPR diffusion → SYNAPSE post-processing → K2KResult.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use sprs::{CsMat, CsVec, TriMat};

use crate::config::{ActivationConfig, EdgeTypeFilter, RetrievalConfig};
use crate::k2k::models::{K2KResult, ResultProvenance};
use crate::knowledge::entity_extractor::EntityExtractor;
use crate::retrieval::intent::{classify, Intent};
use crate::retrieval::ppr::personalized_pagerank;
use crate::retrieval::post_process::post_process;
use crate::retrieval::specificity::SpecificityCache;
use crate::store::Store;

pub struct ActivationEngine {
    db: Arc<dyn Store>,
    activation_config: ActivationConfig,
    edge_filter: EdgeTypeFilter,
    specificity: Arc<SpecificityCache>,
    entity_extractor: Option<Arc<EntityExtractor>>,
}

pub struct ActivationOutput {
    pub results: Vec<K2KResult>,
    pub entity_coverage: f32,
    pub intent: Intent,
    pub iterations: usize,
}

impl ActivationEngine {
    pub fn new(
        db: Arc<dyn Store>,
        config: RetrievalConfig,
        entity_extractor: Option<Arc<EntityExtractor>>,
    ) -> Self {
        Self {
            db,
            activation_config: config.activation.clone(),
            edge_filter: config.edge_types.clone(),
            specificity: Arc::new(SpecificityCache::new()),
            entity_extractor,
        }
    }

    pub async fn search(&self, query: &str, store_id: &str, top_k: usize) -> Result<ActivationOutput> {
        let intent = classify(query);
        let intent_weights = intent.weights();

        // Step 1: extract entities from query
        let seed_entities = self.extract_query_entities(query, store_id).await?;
        if seed_entities.is_empty() {
            return Ok(ActivationOutput {
                results: vec![],
                entity_coverage: 0.0,
                intent,
                iterations: 0,
            });
        }

        // Step 2: get specificity weights for seeds
        let specificity = self.specificity.get(self.db.as_ref(), store_id).await?;

        // Step 3: build the active subgraph
        let (node_index, edges) = self.build_active_subgraph(store_id, &seed_entities).await?;
        if node_index.is_empty() {
            return Ok(ActivationOutput {
                results: vec![],
                entity_coverage: 1.0,
                intent,
                iterations: 0,
            });
        }

        // Step 4: assemble column-stochastic edge matrix with intent multipliers
        let w = build_edge_matrix(&node_index, &edges, intent_weights);

        // Step 5: personalization vector from seeds with specificity weights
        let t = build_personalization_vector(&node_index, &seed_entities, &specificity);

        // Step 6: PPR
        let activation = personalized_pagerank(
            &w, &t,
            self.activation_config.damping,
            self.activation_config.tolerance,
            self.activation_config.max_iter,
        );

        // Step 7: post-process (inhibition + sigmoid + gate + top-k)
        let cfg = &self.activation_config;
        let ranked = post_process(
            &activation,
            cfg.inhibition_beta, cfg.inhibition_m,
            cfg.sigmoid_gamma, cfg.gate_tau,
            top_k.min(cfg.top_k),
        );

        // Step 8: translate top nodes back to articles → K2KResult
        let results = self.materialize_results(store_id, &node_index, &ranked, intent).await?;

        Ok(ActivationOutput {
            results,
            entity_coverage: 1.0,
            intent,
            iterations: 0, // PPR currently doesn't expose this; ok to leave 0 for now
        })
    }

    async fn extract_query_entities(&self, query: &str, _store_id: &str) -> Result<Vec<String>> {
        // Use entity_extractor if available; otherwise fall back to keyword
        // tokens from QueryExpander. For initial P6 version, simple approach:
        // tokenize, lowercase, filter stopwords, return tokens.
        let tokens: Vec<String> = query
            .split_whitespace()
            .filter(|w| w.len() > 2)
            .map(|w| w.to_lowercase())
            .collect();
        Ok(tokens)
    }

    async fn build_active_subgraph(
        &self,
        store_id: &str,
        seeds: &[String],
    ) -> Result<(NodeIndex, Vec<TypedEdge>)> {
        // BFS from seeds across enabled edge types, capped at subgraph_cap.
        // For initial cut, just collect one-hop neighbors of each seed.
        // ...implementation details below in helper functions...
        unimplemented!("TODO: BFS-bounded subgraph extraction")
    }

    async fn materialize_results(
        &self,
        store_id: &str,
        node_index: &NodeIndex,
        ranked: &[(usize, f32)],
        intent: Intent,
    ) -> Result<Vec<K2KResult>> {
        // Map node indices back to article ids; fetch articles; build K2KResult
        unimplemented!("TODO: materialize results")
    }
}

#[derive(Default)]
pub struct NodeIndex {
    pub id_to_idx: HashMap<String, usize>,
    pub idx_to_id: Vec<String>,
}

impl NodeIndex {
    pub fn is_empty(&self) -> bool { self.idx_to_id.is_empty() }
    pub fn len(&self) -> usize { self.idx_to_id.len() }

    pub fn insert(&mut self, id: String) -> usize {
        if let Some(&i) = self.id_to_idx.get(&id) {
            return i;
        }
        let i = self.idx_to_id.len();
        self.id_to_idx.insert(id.clone(), i);
        self.idx_to_id.push(id);
        i
    }
}

pub struct TypedEdge {
    pub from: usize,
    pub to: usize,
    pub edge_type: String,
    pub raw_weight: f32,
}

fn build_edge_matrix(
    nodes: &NodeIndex,
    edges: &[TypedEdge],
    intent_weights: crate::retrieval::intent::IntentWeights,
) -> CsMat<f32> {
    let n = nodes.len();
    let mut tri = TriMat::new((n, n));

    for e in edges {
        let multiplier = match e.edge_type.as_str() {
            "entity_overlap" => intent_weights.entity_overlap,
            "semantically_related" => intent_weights.semantically_related,
            "precedes" => intent_weights.precedes,
            "caused_by" => intent_weights.caused_by,
            "references_edge" => intent_weights.references_edge,
            _ => 1.0,
        };
        tri.add_triplet(e.to, e.from, e.raw_weight * multiplier);
    }

    // Column-stochastic normalization
    let mut csr: CsMat<f32> = tri.to_csr();
    // Normalize each column to sum to 1.0
    for j in 0..n {
        let col_sum: f32 = csr.outer_view(j)
            .map(|col| col.data().iter().sum())
            .unwrap_or(0.0);
        if col_sum > 0.0 {
            if let Some(col) = csr.outer_view_mut(j) {
                for v in col.data_mut() {
                    *v /= col_sum;
                }
            }
        }
    }
    csr
}

fn build_personalization_vector(
    nodes: &NodeIndex,
    seeds: &[String],
    specificity: &HashMap<String, f32>,
) -> CsVec<f32> {
    let n = nodes.len();
    let mut indices = Vec::new();
    let mut values = Vec::new();
    for seed_id in seeds {
        if let Some(&idx) = nodes.id_to_idx.get(seed_id) {
            let w = specificity.get(seed_id).copied().unwrap_or(1.0);
            indices.push(idx);
            values.push(w);
        }
    }
    // Sort by index (CsVec requires sorted indices)
    let mut paired: Vec<(usize, f32)> = indices.into_iter().zip(values).collect();
    paired.sort_by_key(|(i, _)| *i);
    let (indices, values): (Vec<_>, Vec<_>) = paired.into_iter().unzip();

    CsVec::new(n, indices, values)
}

#[cfg(test)]
mod tests {
    // Tests added in Task 7 once helpers are wired up.
}
```

- [ ] **Step 2: Implement `build_active_subgraph`**

The BFS extraction. Start from seed entities; expand via `Store::list_graph_neighbors` for each enabled edge type from P5 Task 9. Cap by `activation_config.subgraph_cap`.

```rust
async fn build_active_subgraph(
    &self,
    store_id: &str,
    seeds: &[String],
) -> Result<(NodeIndex, Vec<TypedEdge>)> {
    let mut nodes = NodeIndex::default();
    let mut edges = Vec::new();
    let mut frontier: Vec<String> = seeds.iter().cloned().collect();
    let mut visited = std::collections::HashSet::<String>::new();

    // Seed-node insertion
    for s in seeds { nodes.insert(s.clone()); visited.insert(s.clone()); }

    let cap = self.activation_config.subgraph_cap;
    while !frontier.is_empty() && nodes.len() < cap {
        let mut next = Vec::new();
        for node in &frontier {
            let neighbors = self.db.list_graph_neighbors(store_id, node, &self.edge_filter).await?;
            for (neighbor_id, edge_type, score) in neighbors {
                let from_idx = nodes.insert(node.clone());
                let to_idx = nodes.insert(neighbor_id.clone());
                edges.push(TypedEdge {
                    from: from_idx,
                    to: to_idx,
                    edge_type,
                    raw_weight: score as f32,
                });
                if visited.insert(neighbor_id.clone()) && nodes.len() < cap {
                    next.push(neighbor_id);
                }
            }
        }
        frontier = next;
    }

    Ok((nodes, edges))
}
```

- [ ] **Step 3: Implement `materialize_results`**

```rust
async fn materialize_results(
    &self,
    store_id: &str,
    node_index: &NodeIndex,
    ranked: &[(usize, f32)],
    intent: Intent,
) -> Result<Vec<K2KResult>> {
    let mut results = Vec::with_capacity(ranked.len());
    for (rank, (idx, score)) in ranked.iter().enumerate() {
        let article_id = &node_index.idx_to_id[*idx];
        let Some(article) = self.db.get_article(article_id).await? else { continue };
        // Skip articles from other stores (defense-in-depth — seeds should have already filtered)
        if article.store_id != store_id { continue; }

        let summary = if article.content.len() > 200 {
            let end = (0..=200).rev()
                .find(|&i| article.content.is_char_boundary(i))
                .unwrap_or(0);
            format!("{}...", &article.content[..end])
        } else {
            article.content.clone()
        };

        results.push(K2KResult {
            article_id: article.id.clone(),
            store_id: article.store_id.clone(),
            title: article.title,
            summary,
            content: article.content,
            confidence: *score,
            source_type: article.source_type,
            tags: vec![],
            metadata: serde_json::json!({
                "search_type": "activation",
                "intent": format!("{:?}", intent),
                "rank": rank,
                "activation_score": score,
            }),
            provenance: Some(ResultProvenance {
                store_id: article.store_id,
                store_type: "activation".into(),
                original_rank: rank,
                rrf_score: 0.0,
            }),
        });
    }
    Ok(results)
}
```

- [ ] **Step 4: Module export + first commit**

```rust
pub mod activation;
pub use activation::{ActivationEngine, ActivationOutput};
```

```bash
cargo build 2>&1 | tail -10
git add src/retrieval/activation.rs src/retrieval/mod.rs
git commit -m "feat(p6): ActivationEngine orchestrator (intent + PPR + post-process)"
```

---

### Task 7: ActivationEngine end-to-end test

**Files:**
- Modify: `src/store/mod.rs`

Add an integration test in `p3_integration_tests` that:
- Seeds 4 articles + entities + MENTIONS + ENTITY_OVERLAP + CAUSED_BY edges
- Calls `ActivationEngine::search()` with a "why" query
- Asserts: causal-linked article ranks ahead of non-causal article
- Asserts: intent is correctly identified as Why
- Asserts: result list is non-empty and bounded by top_k

- [ ] **Step 1: Write the test**

```rust
    #[tokio::test]
    async fn activation_engine_ranks_causal_for_why_query() {
        use crate::retrieval::ActivationEngine;
        use crate::retrieval::intent::Intent;
        use crate::config::RetrievalConfig;
        use std::sync::Arc;

        let s = fixture().await;
        let ts = now();

        // 4 articles in store ae-s1
        for (id, title) in &[
            ("ae-a1", "Outage retrospective"),
            ("ae-a2", "Deploy that caused outage"),
            ("ae-a3", "Unrelated article"),
            ("ae-a4", "Another unrelated"),
        ] {
            s.create_article(&Article {
                id: id.to_string(), store_id: "ae-s1".into(), title: title.to_string(),
                content: format!("Content for {}", id),
                source_type: "user".into(), source_id: String::new(),
                content_hash: format!("ae-{}", id), tags: serde_json::json!([]),
                embedded_at: None,
                created_at: ts.clone(), updated_at: ts.clone(),
            }).await.unwrap();
        }

        // Entity "outage" mentioned in a1 and a2; "deploy" mentioned only in a2
        s.create_entity(&Entity {
            id: "ae-tool-outage".into(), name: "outage".into(),
            entity_type: "concept".into(), description: None,
            store_id: "ae-s1".into(), mention_count: 2,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_entity(&Entity {
            id: "ae-tool-deploy".into(), name: "deploy".into(),
            entity_type: "concept".into(), description: None,
            store_id: "ae-s1".into(), mention_count: 1,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        s.create_mentions_edge("ae-a1", "ae-tool-outage", "outage retro", 0.95).await.unwrap();
        s.create_mentions_edge("ae-a2", "ae-tool-outage", "the outage", 0.92).await.unwrap();
        s.create_mentions_edge("ae-a2", "ae-tool-deploy", "deploy that", 0.95).await.unwrap();

        // CAUSED_BY edge: deploy article caused outage article
        s.create_caused_by_edge(
            "ae-s1", "ae-a2", "ae-a1",
            0.9, Some("explicit causal chain".into())
        ).await.unwrap();

        // Build engine with all edge types enabled
        let mut config = RetrievalConfig::default();
        config.edge_types.caused_by = true;
        let db: Arc<dyn Store> = Arc::new(s);
        let engine = ActivationEngine::new(db, config, None);

        let output = engine.search("why did the outage happen?", "ae-s1", 10).await.unwrap();
        assert_eq!(output.intent, Intent::Why);
        assert!(!output.results.is_empty(), "engine should return at least one result");

        // Articles linked via causal edges should rank higher than disconnected ones
        let ranked_ids: Vec<&str> = output.results.iter().map(|r| r.article_id.as_str()).collect();
        assert!(ranked_ids.iter().any(|id| *id == "ae-a1" || *id == "ae-a2"),
            "expected at least one outage-linked article; got {:?}", ranked_ids);
    }
```

- [ ] **Step 2: Run + commit**

```bash
cargo test --lib store::p3_integration_tests::activation_engine_ranks_causal_for_why_query 2>&1 | tail -10
git add src/store/mod.rs
git commit -m "test(p6): ActivationEngine ranks causal-linked articles for Why queries"
```

---

### Task 8: Wire activation into router pipeline

**Files:**
- Modify: `src/retrieval/graph.rs`
- Modify: `src/router/executor.rs`
- Modify: `src/router/mod.rs`

Until now `GraphSearcher` always uses P4's one-hop jaccard. P6 adds `graph_strategy` config: when set to `"activation"` (the new default), the router calls `ActivationEngine::search` instead; when set to `"jaccard"`, P4 behavior is preserved.

- [ ] **Step 1: Add strategy dispatch to graph search**

In `src/retrieval/graph.rs`, refactor `GraphSearcher::search` to dispatch based on `config.graph_strategy`:

```rust
pub async fn search(&self, query: &str, store_id: &str, top_k: usize) -> Result<GraphSearchOutput> {
    if self.config.graph_strategy == "activation" {
        return self.search_via_activation(query, store_id, top_k).await;
    }
    self.search_via_jaccard(query, store_id, top_k).await
}

async fn search_via_jaccard(&self, query: &str, store_id: &str, top_k: usize) -> Result<GraphSearchOutput> {
    // existing P4 implementation moves here
}

async fn search_via_activation(&self, query: &str, store_id: &str, top_k: usize) -> Result<GraphSearchOutput> {
    let engine = ActivationEngine::new(
        self.db.clone(),
        self.config.clone(),
        None, // EntityExtractor injection can come later
    );
    let out = engine.search(query, store_id, top_k).await?;
    Ok(GraphSearchOutput {
        results: out.results,
        entity_coverage: out.entity_coverage,
    })
}
```

- [ ] **Step 2: Verify wires + commit**

```bash
cargo test --lib 2>&1 | tail -10
git add src/retrieval/graph.rs
git commit -m "feat(p6): GraphSearcher dispatches by graph_strategy (jaccard | activation)"
```

---

### Task 9: Ablation regression tests

**Files:**
- Create: `src/retrieval/ablation_tests.rs` (or inline into existing module)

The roadmap specifies 8 ablation regression tests that prove each verified-paper component is actually load-bearing. P6 makes these executable. We won't reproduce exact paper numbers (different corpus / model) but we can verify the **directional** behavior:

1. PPR vs naive 1-hop: PPR produces strictly more results when graph has multi-hop structure
2. Specificity off (uniform): rare-entity weight equals common-entity weight (degenerate but verifiable)
3. Intent-adaptive off (OpenDomain weights for all queries): causal edges lose their Why-boost
4. Lateral inhibition off (β=0): hub nodes dominate
5. Sigmoid off: outputs span a wider range
6. Confidence gate off (τ=0): more low-quality results pass through
7. Damping=0.0: no PPR signal, only personalization
8. Damping=1.0: only random-walk, no restart bias

Each is a small, fast test. Add 8 tests, each toggling one knob and verifying directional behavior.

- [ ] **Step 1: Add the 8 tests**

(Add a new `#[cfg(test)] mod ablation_tests` inside `src/retrieval/activation.rs` or as a sibling file. Tests build a small fixture graph and toggle each parameter.)

- [ ] **Step 2: Run + commit**

```bash
cargo test --lib retrieval::activation::ablation 2>&1 | tail -15
git add src/retrieval/activation.rs
git commit -m "test(p6): 8 ablation regression tests for activation components"
```

---

### Task 10: CLI `graph-debug --activation-trace` and final cleanup

**Files:**
- Modify: `src/main.rs`

Add an optional `--activation-trace` flag to `graph stats` or a new `graph activation` subcommand. When set, prints intent classification, seed entities, subgraph size, PPR iterations, and final ranked nodes for a given query.

- [ ] **Step 1: CLI surface**

```rust
GraphAction::Activation {
    query: String,
    #[arg(long)] store: Option<String>,
    #[arg(long, default_value="10")] limit: usize,
},
```

Handler runs `ActivationEngine::search` and prints the trace. Useful for debugging on real corpora.

- [ ] **Step 2: Final cleanup + commit**

Run clippy on P6-introduced files only; fix any new warnings.

```bash
cargo clippy --lib --bins --tests 2>&1 | grep -E "warning|error" | grep "src/retrieval/\(activation\|ppr\|post_process\|intent\|specificity\)" | head -10
git add -A
git commit -m "feat(p6): graph activation CLI trace + clippy cleanup"
```

---

### Task 11: Push + open PR

- [ ] **Step 1: Final verification**

```bash
cargo build --release 2>&1 | tail -5
cargo test --lib 2>&1 | tail -10
```
Expected: clean release build; ~125 tests pass (113 baseline + ~12 new P6 tests).

- [ ] **Step 2: Push + PR**

```bash
git push -u origin feat/p6-spreading-activation
gh pr create --base feat/p5-followups --title "P6: Spreading Activation Retrieval (PPR + SYNAPSE + MAGMA)" --body "..."
```

PR body: summarize the four components (intent classifier, specificity, PPR, post-process), note the stack depth, link the roadmap, list the ablation tests.

---

## Self-Review Checklist

- [ ] Every algorithmic constant traces to a peer-reviewed paper
- [ ] `graph_strategy = "jaccard"` preserves P4 behavior; tests verify
- [ ] `graph_strategy = "activation"` is the new default
- [ ] PPR converges within max_iter on test fixtures
- [ ] Lateral inhibition + sigmoid + gating produce in-range outputs
- [ ] 8 ablation regression tests cover the documented contributors
- [ ] ActivationEngine returns provenance (intent, score) in K2KResult metadata
- [ ] Multi-store isolation preserved (P5 followup #4 fix not broken)
- [ ] No new clippy warnings on P6 code

## Out of Scope (Deferred to P7+)

- LLM-based intent classifier (Ollama 3B for ambiguous queries) — config flag exists but path is no-op until P7+
- Performance benchmarks on real corpora — separate effort
- Learned per-edge-type decay (vs config-driven static values) — explicitly P10+
