# Supermemory Upgrade Roadmap (Phases P5–P9)

> **Status:** Strategic roadmap. Each numbered phase below will spawn its own detailed bite-sized implementation plan (P3/P4 format) when scheduled. This document is the umbrella that fixes the direction, scope boundaries, and sequencing.
>
> **Assumed baseline:** P1 (SurrealDB data layer), P2 (pluggable VectorQuantizer), P3 (entity extraction + dedup + knowledge graph) are merged. P4 (graph-powered tri-signal retrieval) is specced; if not yet merged when P5 starts, P4 is executed first as already specced.

**Goal:** Bring KNL to supermemory-class agent memory while keeping its existing advantages (local-first, offline-capable, federation-ready, provider-agnostic). Add temporal/causal structure, query-time activation, event segmentation, consolidation, decay-with-quarantine, and an agent-native API.

**Architecture in one paragraph:** Today KNL is a hybrid retrieval store with an entity graph bolted on. The five phases below transform it into an *agent memory* in the 2026 sense of the term: a system that writes (observe), manages (segment, reflect, decay), and reads (activation over a typed multi-graph) under an agent-shaped API — without giving up its local-first substrate or sending content off the node.

**Tech Stack:** Existing — Rust, SurrealDB (RELATE/edge tables/FTS), LanceDB (vectors), Ollama (local LLM), ONNX embeddings, K2K federation. No new substrates; all phases reuse what's in tree.

---

## Design Axioms (must not be violated by any phase)

These are durable constraints that any phase's design must satisfy. They override convenience.

1. **Local-first, offline-capable.** Default install must function with no network. LLM-dependent features must have heuristic fallbacks and must work with any model Ollama can host, including small ones (3B-class).
2. **Provider-agnostic LLM.** No phase may hardcode a specific provider or model. All prompts must be parameterizable; all parsing must tolerate degraded outputs from small models. (See `[[feedback_no_hardcoded_providers]]`.)
3. **Federation-preserving.** All schema and API changes must keep K2K (multi-node, RSA-JWT) working. Nodes remain sovereign; cross-node memory operations are explicit and consent-gated.
4. **Pluggable substrate.** Quantizer, edge extraction strategy, activation policy, decay function — each is a trait with at least one swappable impl. No phase introduces an un-overridable behavior.
5. **Privacy by default.** No telemetry. Content does not leave the node implicitly. Local LLM calls are local; cross-node fan-out is opt-in per query.
6. **Quarantine, never delete.** Destructive-feeling operations require explicit user confirmation and a recovery path. Decay tiers down (Hot→Warm→Cold→Archive); compaction summarizes without erasing sources. Deletion is an admin operation, never automated.
7. **AI-native, not human-org metaphor.** Memory structures are derived from machine cognition needs (activation, decay, consolidation) — not from "departments," "filing cabinets," or other anthropomorphic org metaphors. (See `[[feedback_ai_native_design]]`.)
8. **No LLM in the retrieval hot path.** LLM calls happen only at ingest, consolidation, and reflection — never during recall. Evidence: MemoryOS (arXiv 2506.06326) hits 32.68s/query because 4.9 LLM calls execute synchronously during retrieval; SYNAPSE and MAGMA stay at 1.5-1.9s by calling LLMs only at write time. The optional query-intent classifier (P6) may call an Ollama 3B model for ambiguous queries, but must be cacheable and async-able with a rule-based fast path covering ≥80% of queries. This axiom is what makes KNL's <1s p50 latency target achievable on local hardware.

---

## Current State (post-P3, P4-pending)

| Capability | State | Module |
|---|---|---|
| Unified data layer (SurrealDB) | ✅ shipped (P1) | `src/store/` |
| Pluggable VectorQuantizer (IVF-PQ default, Int8 alt, TurboQuant stub) | ✅ shipped (P2) | `src/vectordb/quantizer/` |
| Entity extraction (Ollama) + content dedup + knowledge graph | ✅ shipped (P3) | `src/knowledge/`, `src/store/` |
| Tri-signal hybrid retrieval (vector + FTS + graph) with adaptive RRF | 🟡 specced, not merged (P4) | `src/retrieval/`, future `src/retrieval/graph.rs` |
| Federation (K2K) with multi-node RRF merge | ✅ shipped | `src/router/`, k2k crate |
| Embedding: all-MiniLM-L6-v2 (384d, ONNX) | ✅ shipped | `src/embed/` |

**What's not in tree:** anything resembling agent memory in the 2026 sense — no events, no reflections, no decay, no activation, no agent API, no temporal/causal edges.

---

## Gap Analysis (KNL vs. 2026 arXiv frontier)

Grounded in the literature surveyed, with abstracts verified directly:

- **Du (2026)** *Memory for Autonomous LLM Agents: Mechanisms, Evaluation, and Emerging Frontiers* — arXiv **2603.07670** (Mar 2026).
- **Jiang, Li, Li, Li (2026)** *MAGMA: A Multi-Graph based Agentic Memory Architecture for AI Agents* — arXiv **2601.03236** (ACL 2026 Main).
- **Jiang et al. (2026)** *SYNAPSE: Empowering LLM Agents with Episodic-Semantic Memory via Spreading Activation* — arXiv **2601.02744** (Jan 2026).
- **Hu, Liu, Tan, Zhu, Dou (2026)** *Memory Matters More: Event-Centric Memory as a Logic Map for Agent Searching and Reasoning* (framework name: CompassMem) — arXiv **2601.04726** (Jan 2026).
- **Luo et al. (2026)** *From Storage to Experience: A Survey on the Evolution of LLM Agent Memory Mechanisms* — arXiv **2605.06716** (May 2026).
- **Lin, Li, Chen (2026)** *A Survey on the Security of Long-Term Memory in LLM Agents: Toward Mnemonic Sovereignty* — arXiv **2604.16548** (Apr 2026).
- **Jiménez Gutiérrez, Shu, Gu, Yasunaga, Su (2024)** *HippoRAG: Neurobiologically Inspired Long-Term Memory for Large Language Models* — arXiv **2405.14831** (NeurIPS 2024). Personalized PageRank as principled spreading activation; PPR contributes +11pp in their ablation; naive 1-hop expansion HURTS by -13pp.
- **Kang, Ji, Zhao, Bai (2025)** *Memory OS of AI Agent* (MemoryOS) — arXiv **2506.06326** (May 2025, EMNLP 2025 Oral). Heat-based tier promotion formula; verified 32.68s/query latency anti-pattern (synchronous LLM in retrieval).
- **Ma, Nan, Wu, Chen (2025)** *Nemori: Self-Organizing Agent Memory Inspired by Cognitive Science* — arXiv **2508.03341** (Aug 2025; rev. Apr 2026). Open-source at github.com/nemori-ai/nemori. Predict-Calibrate distillation; Two-Step Alignment event segmentation; LoCoMo 80.8 / LongMemEval 64.2-74.6 (different evaluation methodology than SYNAPSE/MAGMA — see caveat below).
- Benchmark suite: LoCoMo, LongMemEval (MAGMA, Nemori), NarrativeQA (CompassMem), plus MemoryArena / MemoryAgentBench from the Du survey.

**Benchmark methodology caveat.** Reported LoCoMo F1 across the bibliography spans 33.3 (A-MEM) to 80.8 (Nemori) — these are NOT apples-to-apples comparisons. Backbone model varies (GPT-4.1-mini, GPT-4o-mini, Qwen2.5-14B), judge protocol varies (string-match F1 vs. LLM-as-judge vs. multiple-choice accuracy), and dataset subsets vary. KNL's target (≥38 F1 against SYNAPSE's 40.5) uses SYNAPSE's exact evaluation protocol; targeting Nemori's 80.8 would require matching its evaluation protocol, which we have not yet validated. Treat the bibliography numbers as within-paper ranks, not cross-paper absolutes.

| Frontier capability | KNL today | Phase |
|---|---|---|
| Multi-graph: separate semantic / entity / temporal / causal views (MAGMA) | Single graph; RELATED_TO mixes entity-overlap with everything | **P5** |
| Spreading-activation retrieval over a typed graph (SYNAPSE) | Static Jaccard RELATED_TO computed eagerly at write-time | **P6** |
| Event segmentation as the unit of memory (CompassMem) | Articles + conversations only, no event abstraction | **P7** |
| Reflection / consolidation (Storage→Experience) | None | **P7** |
| Forgetting policy + tiered salience | None — memory accumulates indefinitely | **P8** |
| Agent-native (observe/recall/reflect) verbs + token-budget responses | Article-centric REST CRUD | **P9** |
| Memory governance primitives (write/read/update/forget authorization) | Strong: RSA-JWT, allowlists, dedup, admin localhost-only | mostly already there; tightened in P8/P9 |
| Policy-learned write/read/manage control | None | **Deferred (P10+)** |

Where KNL is genuinely ahead of much of the literature: local-first execution, governance/auth surface, federation. These advantages must be preserved through every phase below.

---

## Maturity Stage Assessment (Luo et al., arXiv 2605.06716)

Luo's three-stage framework gives a precise rubric for where any memory system sits on the maturity ladder. Verbatim definitions:

- **Storage** — "preserves trajectories with minimal transformation, maintaining a one-to-one correspondence between memory entries and execution traces." Raw memory ℳ_raw with chronological observation-action pairs.
- **Reflection** — semantic transformation `ℱ_ref: 𝒯 → 𝒮` that analyzes completed trajectories to generate refined memory m'_i, decoupling valuable logic from trajectory noise. Requires evaluation criteria ϕ.
- **Experience** — cross-trajectory abstraction `ℱ_exp(𝒯_batch) = 𝒦` compressing similar trajectories into universalized rules `|𝒦| ≪ Σ|τ|`. Serves as policy prior for unseen scenarios.

**KNL's current and target maturity:**

| Stage | Today (post-P3) | After P5-P8 | After P9 |
|---|---|---|---|
| **Storage** | ✅ Mature. SurrealDB + LanceDB preserve trajectories verbatim with rich metadata. | ✅ Enhanced. Multi-graph adds temporal/causal/semantic structure on top of preserved trajectories. | ✅ Maintained. |
| **Reflection** | ❌ Absent. No `ℱ_ref` operator. | 🟡 **P7 introduces this.** Reflection job is the operator; cluster-detection + LLM summarization is the implementation; evaluation criteria ϕ = compression-amplified-toxin defense (min source confidence). | ✅ Available via `/v1/memory/reflect` on-demand. |
| **Experience** | ❌ Absent. | ❌ Still absent. Reflections are trajectory-scoped, not cross-trajectory abstracted rules. | ❌ Still absent. **Deferred to P10+.** |

**Strategic implication.** P5-P9 lands KNL solidly in the Reflection stage with strong Storage foundations and selective Experience-adjacent capabilities (P9's `follow_ups` is a small step toward Active Exploration). Experience-stage capabilities — cross-trajectory rule abstraction with `|𝒦| ≪ Σ|τ|` — are deferred because Luo identifies their state as "nascent" with no established benchmarks; we wait until the field has clearer evaluation primitives before committing to an architecture.

---

## Coverage of Du's Open Problems (Du, arXiv 2603.07670 §9)

Du's survey enumerates ten open problems for agent memory. KNL's planned coverage:

| # | Open problem | KNL coverage |
|---|---|---|
| 9.1 | Principled consolidation (balance hoarding vs. amnesia; offline consolidation during idle periods) | **P7** reflection job + **P8** compaction-via-reflection, both run during idle periods |
| 9.2 | Causally grounded retrieval (beyond semantic similarity) | **P5** `CAUSED_BY`/`ENABLES` edges + **P6** intent-adaptive policy boosts causal edges for Why queries (MAGMA Table 6: w_causal up to 5.0) |
| 9.3 | Trustworthy reflection (validate reflections, quantify uncertainty, challenge stored beliefs) | **P7** records reflection confidence = min(source confidences); **P9** audit log enables challenge; conflict detection via P3 dedup queue |
| 9.4 | Learning to forget (selective forgetting under safety constraints) | **P8** salience-based tiering + quarantine-not-delete; **learned** form deferred to P10+ (training data unavailable today) |
| 9.5 | Multimodal & embodied memory | **OUT OF SCOPE** for KNL. This belongs to Animus, which has perception/embodiment layers. |
| 9.6 | Multi-agent memory governance | **P9** K2K federation governance (Share lifecycle phase); Lin et al.'s 9 primitives mapped in P9 |
| 9.7 | Memory-efficient architectures (sparse retrieval, compressed vectors) | Already shipped: **P2** pluggable VectorQuantizer (IVF-PQ, Int8, TurboQuant stub). P6 sparse-PPR + Top-K=15 edge pruning further reduce footprint. |
| 9.8 | Deeper neuroscience integration (spreading activation, reconsolidation, Ebbinghaus curves) | **P6** PPR + SYNAPSE post-processing = spreading activation; **P7** reflection = reconsolidation; **P8** Ebbinghaus-curve decay available as opt-in |
| 9.9 | Foundation models for memory management (task-agnostic trained controller) | **DEFERRED to P10+** (per [[feedback_no_hardcoded_providers]], we cannot bake in a specific FM; awaits open-weights small specialist models) |
| 9.10 | Standardized evaluation (GLUE-style shared leaderboard for agent memory) | KNL's success metrics already target LoCoMo, LongMemEval, NarrativeQA; if a community leaderboard emerges, KNL will compete on it |

**8 of 10 covered by P5-P9.** The two uncovered (multimodal, learned-FM-controller) are intentional scope decisions, not gaps.

---

## Phase Map (one paragraph each)

- **P5 — Decoupled Multi-Graph:** Split the single graph into four edge-typed subgraphs (entity, semantic, temporal, causal). Make GraphSearcher traversal type-aware. Backfill temporal/causal via local LLM with heuristic fallback.
- **P6 — Spreading Activation Retrieval:** Replace static Jaccard with query-time activation propagation over the multi-graph, with edge-type-specific decay and lateral inhibition. The Search Assumption goes away.
- **P7 — Event Segmentation & Reflection:** Add Event as a first-class memory node, segment conversations into events, run scheduled reflection passes that consolidate clusters into summary memories. Reflections are themselves memory.
- **P8 — Forgetting, Decay, Compaction:** Track access, compute salience, demote to Warm/Cold/Archive tiers, compact redundant clusters into reflections. Never deletes; pinnable.
- **P9 — Agent-Native Memory API:** Expose `/v1/memory/observe`, `/v1/memory/recall`, `/v1/memory/reflect`, `/v1/memory/timeline`, `/v1/memory/forget` (soft) — token-budget aware, streaming, federation-aware.

**Deferred (P10+):** Policy-learned memory control. No usage traces exist yet; P9's API logging is the dataset that unlocks this work later.

**Side-track (any phase):** Embedding model upgrade evaluation (nomic-embed-text 768d, BGE-small-en-v1.5, gte-small). Doesn't block; runs behind config flag with A/B.

---

## Sequencing & Dependencies

```
P4 (graph-powered retrieval, prerequisite)
  │
  ▼
P5 (multi-graph)
  │
  ├──▶ P6 (activation) ◀──── P8 decay function (tight coupling — see below)
  │         │
  │         └──▶ P9 (agent API)
  │
  └──▶ P7 (events + reflection)
              │
              └──▶ P9
```

P5 is the keystone — P6, P7, and most of P8 all depend on it. **P6 and P8 are tightly coupled, not sequential:** the decay function (`salience = importance · exp(-λ · days_since_access)`) must be wired into P6's activation propagation step before P6 can match its published targets (SYNAPSE ablation: removing decay drops Temporal F1 by 72%). The tier-transition background job, audit log, and compaction can land later in P8, but the decay function itself is a P6 prerequisite. P9 is last because it's most useful when sitting on top of activation + events + decay + governance.

---

## Cross-Cutting Concerns

These apply to every phase and must be checked at design time, not retrofitted.

- **Migrations:** Every schema change goes through `src/store/migrations.rs` with version bump. Each migration must be tested with a small fixture corpus. Roll-forward only is acceptable (we're pre-1.0); rollback path documented.
- **Federation:** Any new edge type or memory node type must round-trip through K2K serialization. Cross-node recall (P9) is opt-in per query; nodes can refuse to participate.
- **LLM degradation:** Every LLM-dependent step needs a documented heuristic fallback for the no-LLM and small-LLM (3B) cases. Tests cover both paths.
- **Provenance:** Every derived artifact (RELATED_TO edge, reflection, event boundary, decay tier change) must record `extraction_method` (Heuristic | LLM | UserAsserted | Derived) and `confidence` where applicable. The user must be able to filter "heuristic-only" if they want determinism.
- **Performance budgets:** Recall p95 ≤ 300ms local, ≤ 800ms federated 3-node, on a 50k-article corpus. Every phase that touches the read path must be benchmarked against this budget.
- **Quality targets (benchmark-grounded, from verified 2026 papers):**
  - LoCoMo F1: ≥ 40 (SYNAPSE 40.5, Zep 39.7, A-MEM 33.3 — KNL should be top-3 within 12 months of P9).
  - Adversarial robustness: ≥ 80 F1 on adversarial subset (SYNAPSE 96.6, A-MEM 50.0 — KNL targets the SYNAPSE line, not the baseline).
  - Tokens/query: ≤ 1000 on average (SYNAPSE 814, MAGMA 3370, full-context ~16k — local LLM cost matters more than for cloud baselines).
  - Query latency end-to-end: ≤ 1s p50 local (SYNAPSE 1.9s, MAGMA 1.47s — KNL's local-first advantage must show here).
- **Graph size and pruning budget:** SYNAPSE caps |V| ≤ 10,000 to maintain latency. KNL must enforce per-node graph caps (configurable, default 50k articles + linked entities/events) and prune by tier (P8) when exceeded. Federation must not balloon the active graph.

---

## Phase Specifications

Each section below is design-level — enough to scope the phase, decide trade-offs, and spawn a detailed bite-sized plan when scheduled. **No checkboxes here.** The bite-sized plan comes when the phase begins.

### P5 — Decoupled Multi-Graph

**Goal.** Replace the single overloaded graph (MENTIONS / TAGGED / RELATED_TO-via-Jaccard) with four orthogonal edge-typed subgraphs: entity, semantic, temporal, causal. Make all graph traversal (P4's GraphSearcher) edge-type-aware.

**Why this is the keystone.** MAGMA's central finding is that monolithic graphs entangle signals and force the retrieval layer to fudge weights; type-decoupling lets each query select the relational view that actually matches its intent. KNL today entangles "shared entities" with "topically related" with "things-you-might-want-near-this" in a single Jaccard-weighted edge. Without decoupling, P6 (activation) can't decay by edge type, and P7 (events) has nowhere to put "X caused Y" or "X happened before Y."

**Architecture.** Construction strategy mirrors MAGMA's per-graph approach (verified against arXiv 2601.03236): each graph type uses the cheapest extraction method that yields its signal.

- **Entity layer (cheap, deterministic).** Keep `MENTIONS` (article→entity), `TAGGED` (article→tag) unchanged from P3. This is KNL's entity graph; no rebuild needed.

- **Entity-overlap layer (cheap, deterministic).** Rename current `RELATED_TO` → `ENTITY_OVERLAP`. Same Jaccard-on-shared-entities semantics as P3. Disambiguates this signal from true embedding-semantic similarity — they're different relations and should not share a name.

- **True semantic layer (cheap, deterministic, NEW).** Add `SEMANTICALLY_RELATED` as a separate edge: `cos(embedding_i, embedding_j) > θ_sim` (default **θ_sim = 0.92** matching SYNAPSE's association-gate threshold). Reuses existing P1 vector index — no LLM cost. This fills the gap MAGMA's semantic graph fills.

- **Temporal layer (cheap, deterministic).** `PRECEDES` / `FOLLOWS` derived from article/event `created_at` ordering, scoped to entity-overlap clusters (avoids global N² edges). Per MAGMA: this is a "strictly ordered pair (n_i, n_j) where τ_i < τ_j," immutable. No LLM needed.

- **Causal layer (expensive, LLM only).** `CAUSED_BY` / `ENABLES` extracted by Ollama during background consolidation, NOT at write time. Per MAGMA's slow-path consolidation pattern. Confidence-thresholded; sub-threshold edges stored for audit but excluded from retrieval.

- **Citation layer (cheap, user-asserted).** `REFERENCES` — explicit citation the user/source asserted, distinct from any of the above. Only created when source content includes an explicit reference; never inferred.

- **Edge metadata (all types).** `confidence: f32`, `extraction_method: enum {Heuristic, LLM, UserAsserted, Derived}`, `created_at: string`, `store_id: string`. The extraction_method tag is critical for users who want deterministic-only retrieval.

- **Traversal.** P4's `GraphSearcher` becomes type-aware; per-type hop budget. Default config preserves P4 behavior (entity + entity_overlap only) until P6 wires in activation across all types.

- **Backfill, in priority order (per MAGMA cost profile):**
  1. **Temporal (free):** SQL-only backfill from existing `created_at` columns. Bounded edge count via entity-cluster scoping.
  2. **Semantic (cheap):** ANN query over LanceDB index for each article; emit edge if cos > 0.92. Single pass over corpus, ~constant-time per article.
  3. **Citation (cheap, optional):** parse markdown/HTML for explicit links in article content.
  4. **Causal (expensive, LLM-required):** Ollama batch job, idempotent, slow path. Default off; opt-in per `extract-relations --causal`. Heuristic fallback documented: causal extraction does not have an honest heuristic, so the heuristic-only mode simply omits CAUSED_BY edges.

**Schema sketch (`src/store/schema.rs`).**

```sql
-- Rename existing entity-overlap edge (Jaccard on shared entities, from P3)
-- (migration: copies related_to → entity_overlap, drops old)
DEFINE TABLE entity_overlap TYPE RELATION FROM article TO article;
DEFINE FIELD strength ON entity_overlap TYPE float;          -- Jaccard score
DEFINE FIELD confidence ON entity_overlap TYPE float;
DEFINE FIELD extraction_method ON entity_overlap TYPE string;
DEFINE FIELD created_at ON entity_overlap TYPE string;
DEFINE FIELD store_id ON entity_overlap TYPE string;

-- NEW: true embedding-semantic edge (cos > 0.92)
DEFINE TABLE semantically_related TYPE RELATION FROM article TO article;
DEFINE FIELD strength ON semantically_related TYPE float;    -- cosine similarity
DEFINE FIELD confidence ON semantically_related TYPE float;
DEFINE FIELD extraction_method ON semantically_related TYPE string;
DEFINE FIELD created_at ON semantically_related TYPE string;
DEFINE FIELD store_id ON semantically_related TYPE string;

-- Temporal edges (deterministic, free)
DEFINE TABLE precedes TYPE RELATION FROM article TO article;
DEFINE FIELD confidence ON precedes TYPE float;
DEFINE FIELD extraction_method ON precedes TYPE string;
DEFINE FIELD created_at ON precedes TYPE string;
DEFINE FIELD store_id ON precedes TYPE string;

-- Causal edges (LLM, slow path)
DEFINE TABLE caused_by TYPE RELATION FROM article TO article;
-- ...same fields...

-- Citation edges (user-asserted only)
DEFINE TABLE references TYPE RELATION FROM article TO article;
-- ...same fields...

-- Schema version bump: 1.0.0-p3 → 1.0.0-p5
```

**Files (estimated touch).**

| Action | Path | Responsibility |
|---|---|---|
| Modify | `src/store/schema.rs` | DDL for entity_overlap (renamed), semantically_related (new), precedes, caused_by, references; version bump |
| Modify | `src/store/models.rs` | `EntityOverlapEdge` (renamed), `SemanticallyRelatedEdge` (new — cos-sim), `PrecedesEdge`, `CausedByEdge`, `ReferencesEdge` |
| Modify | `src/store/mod.rs` + impl | Typed-edge CRUD; generic `create_edge<E: TypedEdge>`; traversal helpers parameterized by edge type |
| Modify | `src/store/migrations.rs` | P5 migration: rename related_to → entity_overlap; backfill SEMANTICALLY_RELATED via LanceDB ANN; backfill PRECEDES from `created_at` |
| Create | `src/knowledge/semantic_backfill.rs` | Embedding-cosine edge construction (LanceDB ANN scan, threshold 0.92); idempotent |
| Create | `src/knowledge/temporal_backfill.rs` | Deterministic PRECEDES/FOLLOWS from timestamps within entity-clusters; idempotent |
| Create | `src/knowledge/causal_extractor.rs` | LLM-only causal extraction; slow-path consolidation; small-model-tolerant prompt |
| Modify | `src/knowledge/mod.rs` | Export the three new modules |
| Modify | `src/retrieval/graph.rs` (from P4) | Type-aware traversal; per-edge-type hop budgets; per-type weight contribution to adaptive RRF |
| Modify | `src/config/mod.rs` | `[graph]` config: per-edge-type weights, hops, extraction toggles, cos-sim threshold |
| Modify | `src/main.rs` | `extract-relations [--semantic|--temporal|--causal|--citations]` CLI; extend `graph-debug` to show per-edge-type counts |

**Key design decisions (locked in for P5; not up for re-litigation in implementation plan).**

- **Inverse pairs (PRECEDES/FOLLOWS, CAUSED_BY/ENABLES) are stored once and derived on query.** Storing both directions doubles edge count for no information gain. The traversal layer treats them symmetrically.
- **Confidence threshold for inclusion in retrieval is configurable**, default 0.6. Sub-threshold edges are stored (audit + provenance) but excluded from default traversal.
- **Extraction is opt-in per edge type.** A user with no LLM gets entity + semantic + heuristic-temporal. Causal extraction never runs without an LLM (no honest heuristic exists).
- **Backfill is idempotent.** Re-running the extractor on the same corpus produces the same edges (modulo LLM nondeterminism, which is bounded by confidence threshold).

**Success metrics (per-layer, reflecting cost profile).**

- **Temporal backfill** on 1000-article fixture: < 5 seconds (SQL-only, no LLM).
- **Semantic backfill** on 1000-article fixture: < 30 seconds (LanceDB ANN scan).
- **Citation backfill** on 1000-article fixture with markdown sources: < 10 seconds.
- **Causal backfill** on 1000-article fixture: < 5 minutes with `llama3.2:3b` via Ollama (the only LLM-bound step).
- **No-LLM mode** still produces a working graph: entity + entity_overlap + semantically_related + precedes + references all populated; only causal absent.
- `cargo test --lib store::` passes with new edge round-trip tests.
- `graph-debug` CLI shows per-edge-type counts; P4's existing tri-signal RRF tests pass unchanged with default config.
- Federation: a peer node can ingest each new edge type and round-trip it via K2K without schema errors.
- **Edge count sanity:** on a 1000-article fixture, expected counts are within 2× of: entity_overlap ~ existing P3 count; semantically_related ~ 0.5-2× entity_overlap; precedes ~ N²/cluster (bounded); causal ≪ all others.

**Out of scope (P5 explicitly defers).** Spreading activation (→ P6), event nodes (→ P7), decay on edges (→ P8). Embedding upgrade (→ side-track).

**Citations.** MAGMA (Jiang et al., arXiv 2601.03236, ACL 2026 Main) for the decoupling argument — KNL's four edge types directly mirror MAGMA's four-graph view (semantic, temporal, causal, entity). Du (arXiv 2603.07670, Mar 2026) for write-side mechanisms taxonomy. Benchmarks to target: LoCoMo, LongMemEval (both used by MAGMA).

---

### P6 — Spreading Activation Retrieval

**Goal.** Replace eager pre-computed edges (`ENTITY_OVERLAP` Jaccard from P3 + the new `SEMANTICALLY_RELATED` cos-sim from P5) as the dominant graph signal with query-time spreading activation across the P5 multi-graph, with edge-type-specific decay, lateral inhibition, and MAGMA-style query-adaptive intent policy.

**Why.** SYNAPSE (Jiang et al., arXiv 2601.02744, Jan 2026) names this failure mode **"Contextual Tunneling"** and argues that memory must "transcend static vector similarity" and instead be modeled as "a dynamic graph where relevance emerges from spreading activation rather than pre-computed links." KNL's RELATED_TO edges are pre-computed at write time (eager Jaccard from P3); they cannot adapt to query context. SYNAPSE's proposal is in fact remarkably close to KNL's P4 design: *"a Triple Hybrid Retrieval strategy that fuses geometric embeddings with activation-based graph traversal."* That's the same shape as KNL's tri-signal vector + FTS + graph RRF — except the graph signal is *activation*, not static Jaccard. P6 is the upgrade that lands KNL on the Triple Hybrid line. CompassMem (Hu et al., arXiv 2601.04726, Jan 2026) makes the parallel point that retrieval should *navigate* the graph as a "logic map," not merely consume it as ranking signal.

**Architecture.** A synthesis of three verified papers: HippoRAG's Personalized PageRank as the principled activation engine, SYNAPSE's lateral inhibition + temporal decay + gating as post-processing, and MAGMA's query-adaptive policy modulating both edge weights and the personalization vector. Every constant is from a published peer-reviewed source; KNL ships these as defaults and exposes them via config.

**Why PPR + SYNAPSE post-processing, not pure SYNAPSE.** HippoRAG (Jiménez Gutiérrez et al., arXiv 2405.14831, NeurIPS 2024) shows Personalized PageRank IS the mathematically principled formulation of spreading activation, and their ablation specifically measures its contribution at **+11pp** vs. their best non-PPR alternative — by far the largest single contributor. Critically, their ablation also shows **naive 1-hop neighborhood expansion HURTS by -13pp** vs. PPR: spreading activation MUST be multi-hop and smoothed, never just "expand-neighbors." SYNAPSE's iterative dynamics are one valid implementation of activation; PPR is the canonical one with well-understood convergence properties and efficient sparse-matrix implementations. KNL uses PPR for the core diffusion, then applies SYNAPSE's lateral inhibition and gating as post-PPR competition (which PPR alone doesn't provide).

- New `ActivationEngine` (`src/retrieval/activation.rs`).

- **Seed set 𝒯 + personalization vector** (HippoRAG-style). Two changes from naive query-embedding seeding (both ablation-validated):
  1. **Seeds are LLM-extracted query entities, not the full query embedding.** HippoRAG: "extract named entities from the query, encode each, find nearest KG nodes." This dramatically improves multi-hop retrieval where vector similarity over the full query collapses. KNL reuses the existing P3 entity extractor for queries (the same Ollama prompt, applied to query text).
  2. **Specificity weighting** (HippoRAG, ablation +1.7pp): each seed node gets weight `s_i = 1 / |P_i|` where `P_i` is the set of passages/articles linked to node *i*. This is the IDF analogue — common entities get low personalization weight, rare entities get high weight. Drop-in for KNL's existing entity-article edge counts.
  3. Initial activation vector `a^(0)` = specificity-weighted seed entities, zero elsewhere. Reuses P4 hybrid (vector + BM25) hits as additional non-entity seeds when entity extraction yields nothing.

- **Core diffusion: Personalized PageRank** (HippoRAG, ablation +11pp — the dominant contribution). Iterate:

  `a^(t+1) = (1 − d) · 𝒯 + d · W · a^(t)`

  where `d = 0.5` is the damping factor (HippoRAG default — high restart rate keeps the walk local to query concepts), `W` is the column-stochastic edge-weight matrix (constructed from typed edges, see below), and `𝒯` is the personalization vector. Iterate to convergence (typically 30-50 sparse-matrix iterations; truncated at **T_max = 50** with tolerance 1e-4) OR fixed T=3 in low-latency mode. Output: PageRank distribution `a^(*)` over all reachable nodes.

  *Why PPR not naive 1-hop expansion:* HippoRAG ablation directly tests this — naive 1-hop expansion HURTS by **-13pp** vs. PPR. Spreading activation MUST be multi-hop and smoothed; the "just expand neighbors" shortcut is the wrong design.

- **Post-PPR competition: SYNAPSE lateral inhibition + gating** (SYNAPSE-validated post-processing PPR doesn't provide). Applied once after PPR converges, not per-iteration:
  1. **Lateral inhibition** over top-M competitors: `û_i = max(0, a_i^(*) − β · Σ_{k∈T_M}(a_k − a_i) · 𝕀[a_k > a_i])`. Defaults: **β = 0.15, M = 7** (SYNAPSE).
  2. **Sigmoid normalization**: `â_i = σ(γ · û_i)` with **γ = 5.0** (SYNAPSE).
  3. **Confidence gating**: drop results below **τ_gate = 0.12** (SYNAPSE).
  4. **Retention between iterations** (when using fixed T<convergence mode): **δ = 0.5** carries forward (SYNAPSE).

  *Why lateral inhibition matters in KNL:* high-degree entities (e.g., the user's "Rust" entity touching hundreds of articles) cause PPR mass to concentrate on hub nodes. Lateral inhibition prevents winner-take-all and keeps results diverse.

- **Edge weights** (matrix `W` for PPR; column-stochastic after normalization). Per edge type before normalization:
  - Temporal edges (PRECEDES/FOLLOWS): `w_ji = exp(−ρ · |τ_i − τ_j|)`, **ρ = 0.01** (SYNAPSE).
  - Entity-bridge edges (article ↔ article through shared entity, derived via MENTIONS): fixed **w = 0.8** (SYNAPSE's "abstraction" edge weight).
  - SEMANTICALLY_RELATED (cos-sim from P5): `w = sim(h_i, h_j)` if above gate. **Gate decision:** HippoRAG uses **0.8** (their canonical synonymy threshold); SYNAPSE uses **0.92** (stricter, fewer edges, cleaner signal). KNL ships **0.85** as default — midpoint, tunable — with the rationale that local-LLM-extracted entities have noisier embeddings than the cloud-LLM baselines both papers use.
  - CAUSED_BY/ENABLES: default **w = 0.85**. Tune empirically once P5 backfill produces enough edges to measure.
  - REFERENCES (explicit citation): **w = 0.9** (strongest signal — user-asserted, highest confidence).
  - ENTITY_OVERLAP (Jaccard from P3): `w = jaccard(entities_i, entities_j)` with HippoRAG's specificity weighting applied — entities common across many articles contribute less than rare ones.

- **Top-K edge pruning per node: K = 15** (SYNAPSE). Each node retains its top-15 strongest incoming edges; older/weaker edges are dropped at consolidation time. Bounds memory and improves PPR convergence speed. Without this, dense graphs blow out both space and time.

- **Fan-effect awareness via column-stochastic normalization.** PPR's matrix normalization step `W_ji / Σ_k W_jk` is the principled form of SYNAPSE's `fan(j) = deg_out(j)` dilution. Same effect, different formulation. Critical because SYNAPSE ablation shows the fan effect is worth ~9 F1 on Open-Domain.

- **MAGMA-style query-adaptive policy (the single highest-leverage component per MAGMA ablation: 8.9% on LoCoMo).** A lightweight intent classifier maps `q → T_q ∈ {Why, When, Entity, MultiHop, OpenDomain}`. Per-intent edge-type weight overrides applied as multipliers, defaults from MAGMA Table 6 (adapted):

  | Intent | causal | temporal | entity | semantic |
  |---|---|---|---|---|
  | Why | **4.0** | 1.0 | 1.5 | 1.0 |
  | When | 1.0 | **3.0** | 1.0 | 1.0 |
  | Entity | 1.0 | 1.0 | **4.0** | 1.0 |
  | MultiHop | 2.0 | 1.5 | 2.0 | 1.5 |
  | OpenDomain | 1.0 | 1.0 | 1.0 | 1.0 |

  Classifier: rule-based first cut (keyword patterns: "why|because|caused" → Why; "when|after|before|during" → When; named-entity-heavy → Entity; multi-clause + ≥2 entities → MultiHop). Optional Ollama 3B classification call for ambiguous queries (cached). **Not learned in P6** — learned policy stays deferred to P10+. The rule-based first cut captures the bulk of MAGMA's gain.

- **Final fusion** (where the SYNAPSE vs MAGMA design tension lands). KNL keeps the existing P4 RRF for **anchor identification** (preserves the tri-signal pipeline) and adopts SYNAPSE's **linear weighting for the post-activation score**:

  `S(v_i) = λ_1 · sim(h_i, h_q) + λ_2 · a_i^(T) + λ_3 · structural(v_i)`

  SYNAPSE defaults: **λ = (0.5, 0.3, 0.2)** — semantic, activation, structural (PageRank or similar). KNL substitutes intent-adaptive `a_i^(T)`: the activation vector already encodes intent via the policy table above.

  *Rationale for this hybrid:* RRF anchor selection is what P4 ships and works well; SYNAPSE's linear final fusion handily beat alternatives in their ablations. Combining the two preserves KNL's existing strengths while landing on the SYNAPSE performance line.

- **Termination and pruning (bounded by construction).**
  - Fixed T = 3 propagation iterations.
  - Confidence gating: reject results with final score < **τ = 0.12** (SYNAPSE's gate threshold for stable rejection).
  - Top-30 final ranking cutoff.
  - Active subgraph cap: **|V| ≤ 10,000** (SYNAPSE bound; KNL exposes as config, prune by P8 tier when exceeded).

- **Output.** Ranked list with `(article_id, activation_score, intent_path, edge_provenance)`. The reranker (P4) and downstream API (P9) get full provenance — every returned article reports the edges that delivered its activation.

- ActivationEngine replaces GraphSearcher as the graph-signal in the tri-signal RRF. A `graph_strategy` config flag (`"jaccard"` | `"activation"`) lets users fall back to P4 behavior. Default: `"activation"`.

**HippoRAG implementation reality-check (gaps KNL must close).** A direct read of the HippoRAG paper revealed several specification gaps that KNL must close ourselves; inheriting "PPR with damping=0.5" is the *algorithm*, not the *implementation*. Concrete gaps and KNL's resolution:

| HippoRAG gap | KNL's resolution |
|---|---|
| **No online insertion** — HippoRAG is batch-only; synonymy edges are computed by global pairwise comparison. Authors explicitly note "scalability still calls for further validation." | KNL designs for incremental insertion from day one: synonymy edges are computed per-new-node against existing nodes only (one-sided scan), Top-K=15 pruning bounds memory growth, periodic rebalancing is a P8 background job. |
| **No PPR convergence tolerance specified** | KNL ships **tolerance = 1e-4** with hard cap **T_max = 50** iterations. Sparse PPR typically converges in 15-25 iterations at this tolerance. |
| **No sparse matrix format specified** — paper mentions igraph (C library) without committing to a representation | KNL uses **CSR (Compressed Sparse Row)** via the `sprs` Rust crate. Native Rust, no FFI complexity, mature. Alternative: hand-rolled COO→CSR for the edge-weight matrix construction with custom column-stochastic normalization step. |
| **Damping=0.5 never ablated vs. standard 0.85** — HippoRAG just states "0.5 works" with no comparison | KNL ships 0.5 as default but documents this as inherited-not-validated. Empirical sweep (0.3, 0.5, 0.7, 0.85) is part of P6 success criteria, not assumed. |
| **τ=0.8 tuned on only 100 MuSiQue training examples** — very thin validation | KNL ships **0.85** (compromise between HippoRAG 0.8 and SYNAPSE 0.92) as a known-tunable. Re-validate on a KNL-representative corpus during P6 execution. |
| **Per-dataset ablation variance is large** — node specificity contributes −2.7 / −0.4 / −3.8 pts on MuSiQue / 2Wiki / HotpotQA; synonymy −1.2 / −1.5 / −2.2. The "+1.7pp" summary in earlier roadmap drafts was an oversimplification. | Refined ablation regression tests below cite per-dataset ranges rather than single numbers. |
| **igraph (C library) recommended in paper** | Avoid: FFI overhead, build complexity, cross-platform issues. KNL implements PPR natively in Rust (~150 LOC for sparse iteration). Easier to integrate with SurrealDB edge queries. |

This list is what makes the difference between "we read HippoRAG and want PPR" and "we can ship PPR in production." Every row is a P6 implementation-plan task waiting to be written.

**Files.**

| Action | Path | Responsibility |
|---|---|---|
| Create | `src/retrieval/activation.rs` | `ActivationEngine`: orchestrates seed extraction, PPR diffusion, post-processing |
| Create | `src/retrieval/ppr.rs` | Personalized PageRank: column-stochastic edge matrix, sparse iteration, convergence detection, damping=0.5 default |
| Create | `src/retrieval/post_process.rs` | Lateral inhibition (β=0.15, M=7), sigmoid (γ=5.0), confidence gate (τ=0.12); operates on PPR output vector |
| Create | `src/retrieval/intent.rs` | Rule-based query intent classifier (Why/When/Entity/MultiHop/OpenDomain); optional Ollama fallback for ambiguous; modulates both edge-weight matrix and personalization vector |
| Create | `src/retrieval/specificity.rs` | HippoRAG-style node specificity weights `s_i = 1/|P_i|`; cached after each P3 entity-extraction pass |
| Modify | `src/retrieval/mod.rs` | Export; trait `GraphRetriever` with two impls (Jaccard, Activation) |
| Modify | `src/retrieval/graph.rs` | Refactor to dispatch by `graph_strategy` config |
| Modify | `src/retrieval/reranker.rs` | Use activation path as a feature; apply final linear-fusion λ weights |
| Modify | `src/router/executor.rs` | Wire activation strategy + intent classification into pipeline |
| Modify | `src/config/mod.rs` | `[activation]` config: SYNAPSE constants as defaults (ρ, β, M, T, γ, λ, τ, |V|, intent table) |
| Modify | `src/main.rs` | Extend `graph-debug` with activation-trace mode (shows iteration-by-iteration activation per node) |

**Key design decisions.**

- **Use peer-reviewed constants as defaults; expose all of them via config.** No invented values where a measurement exists. From SYNAPSE: ρ=0.01, β=0.15, M=7, T=3, γ=5.0, τ=0.12, δ=0.5, top-30, |V|≤10k, λ=(0.5,0.3,0.2), abstraction w=0.8, top-K-edges=15, dormancy ε=0.01, archive W=10. From HippoRAG: PPR damping=0.5, tolerance=1e-4, specificity s_i=1/|P_i|, synonymy gate 0.8. KNL's only invented constant is the synonymy gate compromise (0.85, midpoint between HippoRAG and SYNAPSE) — flagged as the one tunable that needs empirical validation post-launch.
- **PPR is the diffusion mechanism; SYNAPSE's iterative dynamics are not.** HippoRAG ablation: PPR contributes +11pp, by far the dominant component. PPR's mathematical convergence properties and existing sparse-matrix tooling (igraph, custom Rust impl, GraphBLAS) make it the better implementation target than re-implementing SYNAPSE's hand-tuned iterative dynamics. SYNAPSE's lateral inhibition + gating + sigmoid are kept as post-PPR processing, where they uniquely contribute (PPR doesn't have intrinsic competition).
- **Never use naive 1-hop expansion.** HippoRAG ablation: -13pp vs. PPR. This is permanent guidance, not a default we might revisit. Spreading activation must be multi-hop and smoothed; the "expand-neighbors" shortcut is the wrong shape.
- **Fan-effect dilution is mandatory, expressed as PPR's column-stochastic normalization.** Same effect SYNAPSE achieves with `fan(j) = deg_out(j)` dilution. Worth ~9 F1 per SYNAPSE ablation.
- **Intent-adaptive policy is rule-based in P6, learned in P10+.** Rule-based captures MAGMA's 8.9% LoCoMo gain without an offline training pipeline. Intent modulates two things in PPR: edge-weight matrix (per-type multipliers) AND personalization vector (boosting causal-seed nodes for Why queries, temporal-seed nodes for When queries, etc.).
- **Activation results carry full provenance** — every returned article reports the edges, iterations, and intent path that delivered its activation. Non-negotiable for P9's API and for governance (Lin et al. provenance-tagging primitive).
- **Strategy is selectable per query**, not just globally. A `depth` query parameter can override T; `intent_override` can pin a weight profile; defaults still apply.
- **Failure modes are configured in, not designed out.** The ablation-regression test (disabling node-decay reproduces SYNAPSE's catastrophic drop) is permanent — it proves the pipeline is correctly wired AND it documents what happens if a user disables decay in production.

**Success metrics (benchmark-grounded).**

- **LoCoMo F1 ≥ 38** end-to-end (SYNAPSE achieves 40.5 with their full stack; KNL targets within 2 F1 of SYNAPSE on the same benchmark with a local-LLM backbone).
- **LoCoMo Temporal subset F1 ≥ 45** (SYNAPSE 50.1; KNL must clear 45 since this is the subset most affected by decay+temporal-edge handling).
- **LoCoMo Adversarial subset F1 ≥ 80** (SYNAPSE 96.6, A-MEM 50.0 — the gating threshold τ=0.12 and lateral inhibition are responsible for this; KNL inherits them).
- **p95 activation-step latency < 100ms** on 50k-article corpus with T=3 iterations and |V|≤10k cap.
- **Tokens/query (when LLM intent classifier is invoked) < 100 on average** (only ambiguous queries trigger LLM; rule-based handles ~80% per query-shape heuristics).
- **Ablation regression tests (validate pipeline correctness via reproducing published numbers).** Each row below is a permanent CI test: disable the component, confirm KNL exhibits the published regression. If the regression is materially smaller, the component isn't actually doing the work.
  - **Disable node-decay** (via P8 hook): reproduces SYNAPSE Table 3 — Temporal-F1 drops from 50.1 → 14.2 on LoCoMo. The biggest single regression of any component.
  - **Replace PPR with naive 1-hop expansion**: reproduces HippoRAG Table 5 — ≈8-10 pts regression across MuSiQue / 2Wiki / HotpotQA.
  - **Disable specificity weighting** (set all s_i = 1): reproduces HippoRAG Table 5 per-dataset — MuSiQue -2.7, 2Wiki -0.4, HotpotQA -3.8 pts F1. The 2Wiki near-zero result is real and shows specificity matters more on long-tail-entity corpora.
  - **Disable synonymy edges** (skip cos > θ edge construction): reproduces HippoRAG Table 5 per-dataset — MuSiQue -1.2, 2Wiki -1.5, HotpotQA -2.2 pts F1.
  - **Disable intent-adaptive policy** (use uniform edge weights): reproduces MAGMA Table 4 — overall LoCoMo drops by 8.9% (0.700 → 0.637).
  - **Disable fan-effect normalization** (skip column-stochastic step in W): reproduces SYNAPSE Open-Domain regression — 25.9 → 16.8 F1.
  - **Disable causal edges** (skip P5 LLM causal extraction): reproduces MAGMA Table 4 — overall LoCoMo drops by 8.0% (0.700 → 0.644).
  - **Disable temporal edges**: reproduces MAGMA Table 4 — overall LoCoMo drops by 7.6% (0.700 → 0.647).
- Strategy toggleable at runtime; existing P4 RRF tests pass unchanged when `graph_strategy="jaccard"`.

**Out of scope.** Learning the decay parameters (deferred to P10+). For now, decays are configured constants from literature + tuning.

**Citations.** SYNAPSE (Jiang et al., arXiv 2601.02744, Jan 2026) — direct prior art; KNL's tri-signal retrieval after P6 sits on the same Triple Hybrid Retrieval line. CompassMem (Hu et al., arXiv 2601.04726, Jan 2026) — graph-as-logic-map / navigation framing. MAGMA (arXiv 2601.03236) — *query-adaptive* edge-weight selection (rule-based forms are in-scope here; learned policy is deferred to P10+). Du (arXiv 2603.07670) read-side mechanisms taxonomy.

---

### P7 — Event Segmentation & Reflection

**Goal.** Introduce `Event` as a first-class memory node. Segment conversations into events. Run scheduled reflection passes that consolidate clusters of low-level memories into higher-level summary memories. Reflections are queryable memory, not just metadata.

**Why.** CompassMem (Hu et al., arXiv 2601.04726) draws on Event Segmentation Theory to organize memory as an Event Graph: "incrementally segmenting experiences into events" linked "through explicit logical relations," yielding a structure that "serves as a logic map, enabling agents to perform structured and goal-directed navigation over memory." Luo et al. (*From Storage to Experience*, arXiv 2605.06716, May 2026) identifies a Storage → Reflection → Experience progression in agent memory: KNL today sits at the Storage stage (it preserves trajectories), and P7 is the move into Reflection (refining those trajectories into higher-level structure). For a personal / family / multi-source knowledge mesh, *events* ("the AZ trip in March," "the deploy incident last Tuesday") are the natural unit of recall — not raw articles or conversation messages. Without consolidation, memory accretes without becoming wisdom.

**Architecture.** Adopts CompassMem's verified event-segmentation algorithm and relation taxonomy; **explicitly does NOT adopt CompassMem's Explorer** (see "Why we skip the Explorer" below).

- **Event node type** (new `event` table, sibling to `article`). Following CompassMem's node schema:
  - `id`, `title`, `summary` (s), `observation_span` (o — pointer to source content), `started_at`, `ended_at` (τ), `participants: Vec<String>` (π), `store_id`, `source_type` ("conversation" | "manual" | "derived"), provenance.

- **Event ↔ article edge:** `CONTAINS_EVIDENCE` (event → article).

- **Event ↔ event edges:** reuse P5's typed edges. CompassMem's taxonomy of logical relations (verbatim from arXiv 2601.04726) maps cleanly:
  - **causal** → P5 `CAUSED_BY` / `ENABLES`
  - **temporal** → P5 `PRECEDES` / `FOLLOWS`
  - **motivation** → NEW in P7: `MOTIVATES` (event → event); LLM-extracted; lower confidence default
  - **part-of** → NEW in P7: `PART_OF` (event → event, hierarchical composition)

  CompassMem notes the predicate set is open-ended; KNL ships these four plus a `relates_to` extension hatch for user-defined relations (with confidence floor 0.5).

- **Event ↔ entity edges:** events MENTIONS entities (reuse P3 edge).

- **Segmentation** (`src/knowledge/events.rs`): **LLM-prompted, following CompassMem's approach.** CompassMem: "we prompt an LLM to identify events from the input stream and extract their attributes." KNL's small-model-tolerant prompt extracts `{title, summary, started_at, ended_at, participants, evidence_spans}` per event. **Heuristic fallback (KNL's addition, not CompassMem's):** long silence-gaps + topic-shift signals from entity overlap, for the no-LLM case. Yields coarser segments but the schema is identical, so a later LLM pass can refine.

- **Reflection** (`src/knowledge/reflection.rs`): scheduled background job. Finds clusters (entity-overlap + temporal-proximity + P6 activation-density) and writes a `Reflection` summary. Stored as `article` with `source_type="reflection"` + `reflects: Vec<article_id>` field. **Compression-amplified-toxin defense (Lin et al.):** the reflection's confidence is `min(source_confidences)`, NOT `max` — a reflection cannot exceed the trust level of its weakest source.

- **Predict-Calibrate distillation** (Nemori, arXiv 2508.03341, Ma et al., Aug 2025; open-source at github.com/nemori-ai/nemori). A novel write-side technique that addresses a real problem with naive reflection: most clusters of memories are redundant with what the agent already "knows," so summarizing them adds noise without adding signal. Nemori's solution, adapted for KNL:
  1. Before generating a reflection from a cluster of N memories, prompt the local LLM with the existing memory state plus the question "what should this cluster contain, given what we already know?"
  2. The LLM produces a **predicted** reflection.
  3. Then prompt the LLM with the actual cluster contents and ask "what's in the actual cluster that wasn't in the prediction?"
  4. The **prediction-error delta** is what gets stored as the reflection. Predicted-and-confirmed material is discarded as redundant.

  Two consequences for KNL: (a) reflections become smaller and information-dense rather than redundant retellings; (b) the distillation step is naturally idempotent — re-running it on the same cluster produces empty deltas once the memory has been integrated. Nemori reports this reduces reflection-token-cost by 38.7% vs. baseline distillation.

  **Free-Energy-Principle framing.** Nemori frames this as the agent only storing what it failed to predict — direct lift from neuroscience's Free-Energy Principle. Aligns with KNL's `[[feedback_ai_native_design]]` axiom (machine cognition primitives, not human filing-cabinet metaphors).

  **Cost discipline.** Predict-Calibrate adds **one extra LLM call per reflection** (the predict step). Since reflection is already a write-side, scheduled-background operation (not retrieval-hot-path, per axiom 8), this is acceptable. Configurable: `[reflection].use_predict_calibrate = true` (default on with LLM available, off otherwise).

- **Narrative cues** (Nemori). Each event gets a short retrieval handle (≤20 tokens) generated at segmentation time — a "cue" the agent uses to recall the event mid-conversation. Stored as `event.cue: String`, indexed in BM25 (P1) so cue-based recall is essentially free. Solves a real UX problem: a long event narrative is poor for keyword retrieval but a cue is.

- **Scheduler** (`src/maintenance/scheduler.rs`): cron-style background runner. Default cadence: **nightly reflection, on-demand segmentation**. *Why not SYNAPSE's per-5-turns consolidation cadence:* SYNAPSE's unit is conversational turn (~seconds apart); KNL's unit is article ingest (~minutes-to-days apart). Consolidating every 5 articles would over-segment; nightly aligns with the natural rhythm of a personal-knowledge corpus. Cadence is configurable per-store; high-volume stores may opt into hourly. The trigger is rate-based (every N ingests OR nightly, whichever comes first), so a busy store reflects more often.

**Why we skip CompassMem's Explorer (cost rationale).** CompassMem's Explorer is LLM-driven navigation: at each node the LLM decides `{Skip, Expand, Answer}`, with multiple Explorers running in parallel coordinated via priority queue. Measured cost: **~20.87 seconds average per question on LoCoMo, max 65.38s**. That violates KNL's <1s local-latency target by 20×. The good news: P6's spreading activation gets the same multi-hop navigation behavior at SYNAPSE's 1.9s — and SYNAPSE's LoCoMo F1 (40.5) is in CompassMem's ballpark (52.18 with GPT-4o-mini, dropping with smaller models). **KNL adopts CompassMem's writing-time structure (events + relations) and SYNAPSE's reading-time mechanism (activation), getting CompassMem's organizational benefit at SYNAPSE's cost profile.**

**Files.**

| Action | Path | Responsibility |
|---|---|---|
| Create | `src/knowledge/events.rs` | `Event` segmentation: LLM-based + heuristic fallback |
| Create | `src/knowledge/reflection.rs` | Cluster detection + summarization via local LLM |
| Modify | `src/store/schema.rs` | `event` table, `contains_evidence` edge, schema version bump |
| Modify | `src/store/models.rs` | `Event` struct, `ContainsEvidenceEdge` |
| Modify | `src/store/mod.rs` + impl | Event CRUD; event ↔ entity edge linking |
| Modify | `src/knowledge/articles.rs` | `source_type="reflection"` handling; `reflects: Vec<String>` field on article |
| Create | `src/maintenance/scheduler.rs` | Background job runner; safe restart semantics; per-job idempotency keys |
| Modify | `src/main.rs` | CLI: `reflect [--scope=...]`, `segment-events [--since=...]` |
| Modify | `src/config/mod.rs` | `[reflection]` config: cadence, cluster thresholds, summary length, model name |

**Key design decisions.**

- **Reflections never overwrite their source memories.** Sources are retained verbatim; the reflection adds a higher-level layer above. Quarantine principle (axiom 6) applies even here.
- **Reflections are queryable like any article.** They surface in retrieval naturally. Their `source_type="reflection"` lets queries filter them in or out. The reranker may up-weight reflections for high-level queries and down-weight them for fact-recall queries (heuristic via query classification).
- **Provenance is mandatory.** Every reflection has a `reflects: Vec<article_id>` pointing to its source memories. Users can drill down.
- **Reflection is opt-in and degrades gracefully.** With no LLM, reflection is disabled (no honest heuristic for summarization). The system continues to work as a non-consolidating store. With a 3B model, reflections are shorter and less synthesized but still useful.
- **Segmentation boundaries are tunable.** Conservative defaults (long gaps + clear topic shifts) avoid over-segmentation; users can tune for their corpus.

**Success metrics.**

- Conversation segmentation on a 10-conversation fixture produces event boundaries within 1-event of human-labeled ground truth (allowing for legitimate disagreement).
- Reflection job on 1000-article fixture completes <30min with `llama3.2:3b`; produces non-empty, source-cited reflections.
- Reflections surface in `recall` (with provenance) and can be filtered out via query parameter.
- No source memory is mutated or deleted during reflection (audit log proves this).

**Out of scope.**
- Multi-modal event segmentation (screen capture, audio) — Animus territory, not KNL.
- CompassMem's Explorer / LLM-decision-per-node navigation — too expensive (20s/query average); P6 activation substitutes at 100× less cost.
- Cross-document storyline construction beyond P5's PRECEDES/CAUSED_BY + P7's MOTIVATES/PART_OF.
- Iterative query refinement (CompassMem's Planner re-query) — defer until usage shows recall failures that refinement could fix.

**Citations.** CompassMem (Hu et al., arXiv 2601.04726) — event-graph and logic-map navigation. Luo et al. (arXiv 2605.06716) — Storage→Reflection→Experience progression. Du (arXiv 2603.07670) — manage-side mechanisms (reflective self-improvement family). **Nemori (Ma et al., arXiv 2508.03341, open-source github.com/nemori-ai/nemori)** — Predict-Calibrate distillation + narrative cues + Two-Step Alignment event segmentation; the only system in the bibliography that's available as an open-source reference implementation, useful when writing the P7 bite-sized plan.

---

### P8 — Forgetting, Decay, and Compaction

**Goal.** Add access tracking, decay-based salience scoring, and tiered storage (Hot / Warm / Cold / Archive). Compact redundant clusters into reflections. Never delete; pinnable; reversible.

**Why — and why this is performance-critical, not hygienic.** P8 was easy to mis-read as a "hygiene / housekeeping" phase. The verified literature reframes it as the single most important performance component in the entire roadmap.

- **SYNAPSE ablation (arXiv 2601.02744, Table 3):** removing node decay drops Temporal F1 from **50.1 → 14.2** on LoCoMo — a 72% catastrophic collapse, larger than removing any other component including the graph itself. Decay is not a hygiene feature; it's the load-bearing piece of the activation pipeline. **P6 cannot reach its LoCoMo targets without P8.**
- **MAGMA temporal-backbone ablation (arXiv 2601.03236, Table 4):** removing the temporal backbone drops LoCoMo overall by 7.6%. KNL's PRECEDES/FOLLOWS edges (P5) need timestamps to have meaning, and timestamps need decay-weighted scoring to drive retrieval, which is what P8 supplies.
- **Du survey (arXiv 2603.07670)** identifies selective forgetting as a universal weak spot across MemoryArena and MemoryAgentBench: recall-near-perfect systems plummet to 40-60% when forced to *use* memory selectively under decision pressure.
- **KNL has no decay or tiering today**, which means (a) accuracy will degrade as the corpus grows (everything is equally "relevant"), (b) P6's activation will not match SYNAPSE's published numbers (the algorithm is correctly implemented but the decay input is missing), and (c) Temporal queries — which user workflows lean on heavily — will underperform by tens of F1 points.
- **The Animus VectorFS principle — "quarantine, not delete"** — is the governing constraint: we tier and de-prioritize, never erase. Mnemonic Sovereignty's Forget/Rollback governance phase plus Versioning/Snapshots primitive (Lin et al., arXiv 2604.16548) demand the same shape.

**Sequencing implication:** P8 may begin in parallel with P6 once P5 lands. P6 cannot ship to production until P8's decay function is wired into the activation propagation step, even if the full tier-transition job isn't running yet. Treat P8 as a P6 dependency, not a successor.

**Architecture.**

- **New fields on `article` and `event`:**
  - `access_count: i64`
  - `last_accessed_at: String`
  - `importance_score: f32` (computed from entity-degree, citation count, user-pins, recency-on-create)
  - `tier: enum { Hot, Warm, Cold, Archive }`
  - `pinned: bool`
- **Salience function** (`src/maintenance/decay.rs`). Two named, published formulations to draw from; KNL implements both behind a config flag and ships the SYNAPSE-aligned version as default.

  **(a) SYNAPSE-aligned (default)** — activation-driven decay. A node's salience IS its rolling activation across recent recall queries. From SYNAPSE Section 3.1: "Nodes with activation consistently below dormancy threshold **ε = 0.01** for **W = 10** windows are archived." KNL adopts the same constants; "window" = recall pass. This is the cheapest implementation (decay is the activation engine's natural exhaust) and the most directly aligned with P6.

  **(b) MemoryOS heat formula** (arXiv 2506.06326) — explicit access-pattern scoring. `heat = α·N_visit + β·L_interaction + γ·R_recency`, with recency decay constant μ=1e7, promotion threshold τ=5. Maps cleanly to KNL's per-article columns. More transparent to the user but requires a separate update path.

  **(c) Generative Agents formula** (Park et al., named in Du survey 2603.07670) — three-factor weighted mix: `salience = w_rec · recency_decay + w_rel · relevance_to_query + w_imp · importance`, where importance is a self-assessed (LLM-assigned) integer at write time. Most expensive (LLM call at write for importance) and most accurate per Du's analysis. Use as opt-in for high-stakes corpora.

  **(d) MemoryBank Ebbinghaus-curve** (named in Du 9.8) — biologically-inspired forgetting curve `salience = exp(-t/S)` where `S` is reinforced by access. Mathematically similar to (a) but with explicit reinforcement-on-access; defer to follow-up unless empirical results favor.

- **Tier transitions:**
  - Nightly background job demotes by salience thresholds (configurable; defaults aligned with SYNAPSE: Hot ≥ 0.5, Warm ≥ 0.1, Cold ≥ ε=0.01, Archive < ε=0.01 for W=10 consecutive windows).
  - Any retrieval hit promotes to Hot (activation-driven mode); MemoryOS mode promotes when heat crosses τ=5.
  - Pinned items never demote.
- **Retrieval reranker** (extends P4 reranker): applies tier-aware salience as a final weight. Cold/Archive items are still indexed and surfaceable but down-weighted. A query flag `include_archive=true` overrides.
- **Compaction** (`src/maintenance/compaction.rs`): finds very-low-salience clusters with high redundancy (sharing N+ entities), generates a reflection covering them, marks the originals as `compacted_into: reflection_id`. Originals remain queryable with explicit flag but are excluded from default recall.
- **Audit log** (`src/maintenance/audit.rs`, new): every tier change, every compaction, every pin/unpin is logged with timestamp and reason. Append-only, K2K-replicable.

**Files.**

| Action | Path | Responsibility |
|---|---|---|
| Modify | `src/store/models.rs` | New fields on `Article` and `Event` |
| Modify | `src/store/schema.rs` | New columns, schema version bump |
| Modify | `src/store/migrations.rs` | P8 migration: backfill `access_count=0`, `last_accessed_at=created_at`, `importance_score` from entity-degree, `tier=Hot`, `pinned=false` |
| Create | `src/maintenance/decay.rs` | Salience function, tier transition logic |
| Create | `src/maintenance/compaction.rs` | Cluster detection (uses P6 activation), reflection generation (calls P7 reflection), originals marked compacted |
| Create | `src/maintenance/audit.rs` | Audit log table + writer + query CLI |
| Modify | `src/maintenance/scheduler.rs` (from P7) | Register nightly decay job, weekly compaction job |
| Modify | `src/retrieval/reranker.rs` | Tier-aware salience weighting |
| Modify | `src/router/executor.rs` | Record access on every retrieval hit |
| Modify | `src/main.rs` | CLI: `pin <id>`, `unpin <id>`, `decay-status`, `compact [--dry-run]`, `audit-log [--since=]` |
| Modify | `src/config/mod.rs` | `[decay]`: lambda, tier thresholds, compaction redundancy threshold |

**Key design decisions.**

- **Nothing ever truly deletes by automated pass.** Archive is a tier, not a delete. Compacted originals remain queryable via explicit flag. Hard delete is a manual admin operation, audit-logged, and requires `--confirm` plus a recent backup check.
- **Pin overrides everything.** A pinned memory never demotes, never gets compacted, and gets a permanent salience floor.
- **Access promotes; non-access decays.** This is the only behavioral feedback loop in the system. Tunable but on-by-default.
- **Compaction is dry-run-first.** The first run of compaction on any corpus must show the user what *would* happen. Auto-commit is opt-in.
- **Salience is exposed.** Users can see per-memory salience via CLI. Transparency over magic.

**Success metrics.**

- After 30 days of synthetic access patterns on a 10k-article fixture, salience distribution looks plausible (recent + accessed items in Hot, untouched old items in Cold).
- Tier-aware reranker improves measured recall on "recent and important" queries vs. P4 baseline.
- Audit log captures every tier change and compaction with reversible record.
- Compaction dry-run reports redundancy clusters without writing; user-confirmed run produces reflections with full provenance.
- Hard-delete is impossible via API; only `forget` (soft) is exposed externally.

**Out of scope.** Learned decay parameters (P10+). Cross-node salience reconciliation across federation (P9 federation deals with read fan-out; salience stays node-local per axiom 5).

**Citations.** Du (arXiv 2603.07670) — manage-side: forgetting policies as a mechanism family. LoCoMo / MemoryArena / MemoryAgentBench — benchmark findings on selective forgetting as the universal weak spot. Lin, Li, Chen (*Toward Mnemonic Sovereignty*, arXiv 2604.16548, Apr 2026) — Forget/Rollback as one of six lifecycle phases that must be explicitly governable; "no published architecture covers all nine governance primitives we identify" — KNL's pin + tier + audit-log surface aims to be one that does.

---

### P9 — Agent-Native Memory API

**Goal.** Expose a supermemory-class agent-shaped API: `/v1/memory/observe`, `/v1/memory/recall`, `/v1/memory/reflect`, `/v1/memory/timeline`, `/v1/memory/forget`. Token-budget aware, streaming, federation-aware, auth-preserving.

**Why.** Current API is article-centric REST CRUD designed for human consumers (admin tools, the connect plugin). Agents need verbs that match their cognitive loop: write what just happened, ask for what's relevant given current context, ask the system to reflect, see the timeline, ask to forget. Equally important: an agent has a context window, so a memory API that doesn't enforce token budget is one that crashes the agent at the worst possible moment.

**Architecture.**

- **Namespace:** `/v1/memory/*` added alongside existing endpoints. Existing endpoints unchanged for backward compatibility.
- **Endpoints:**

  - `POST /v1/memory/observe`
    - Body: `{text, modality?: "text"|"file"|"url", source?, ts?, idempotency_key?, async?: bool}`
    - Behavior: dedups, extracts entities (P3), extracts relations (P5), optionally segments into event (P7), returns `{memory_id(s), accepted: bool, reflections_triggered: []}`.
    - Default `async=true` returns immediately; sync mode for tests.

  - `POST /v1/memory/recall`
    - Body: `{query, token_budget: u32, scope?, since?, until?, edge_types?: [...], include_archive?: bool, federate?: bool}`
    - Behavior: runs full pipeline (P4 RRF + P6 activation + P8 salience), packs results into a bundle that fits `token_budget` (truncating/summarizing as needed), returns `{items: [{id, text, score, provenance, tier}], total_budget_used, follow_ups: [...]}`.
    - `follow_ups` are suggested next queries the agent might issue, derived from low-activation neighbors of top hits (a cheap form of curiosity).
    - Streaming SSE supported so the agent can act on first-arriving items.

  - `POST /v1/memory/reflect`
    - Body: `{scope?: {since, until, topic?}, dry_run?: bool}`
    - Behavior: triggers an on-demand reflection (P7) over the scope, returns the reflection memory_id(s).

  - `GET /v1/memory/timeline?since=&until=&topic=&granularity=`
    - Returns chronologically-ordered events + articles for the scope. Uses P5 PRECEDES/FOLLOWS edges as the primary ordering.

  - `POST /v1/memory/forget`
    - Body: `{memory_id, reason}` — soft-archives via P8 (sets tier=Archive, pinned=false), audit-logged. Never hard-deletes.

- **Token-budget enforcement:** items are added in rank order; if next item would exceed budget, it's either truncated, summarized (LLM-assisted with fallback), or dropped, with the bundle reporting `total_budget_used` within 5% of requested.

- **Federation:** `recall` honors `federate: true` by fanning out to peer K2K nodes (existing infra), each returning a bundle; the requesting node merges with RRF and re-applies its own budget. Peer nodes can refuse based on their own policy (axiom 5).

- **Auth:** all `/v1/memory/*` endpoints inherit existing RSA-JWT auth + client allowlist. No new auth surface.

**Files.**

| Action | Path | Responsibility |
|---|---|---|
| Create | `src/api/memory.rs` | Handlers for the 5 endpoints; token-budget bundler; SSE streaming |
| Modify | `src/api/mod.rs` | Route registration |
| Create | `src/api/openapi.rs` (or extend existing) | OpenAPI schema for the new namespace |
| Create | `tests/api/memory_endpoints.rs` | Integration tests per endpoint, including budget enforcement and federation |
| Modify | `src/router/executor.rs` | Plumb `token_budget`, `edge_types`, `include_archive`, `federate` flags through pipeline |
| Modify | `src/main.rs` | Optional: CLI mirrors of the API for local debugging |

**Key design decisions.**

- **Observe is fire-and-forget by default.** The agent shouldn't block on extraction; `accepted: true` is enough. Reflections triggered are returned for awareness only.
- **Recall is read-only and bounded.** Default deadline 1.5s; agent can extend up to 5s. Past deadline, the partial bundle is returned with `partial: true`.
- **Token-budget enforcement is server-side and authoritative.** The agent declares its budget; the server fits within it. If summarization is used to fit, it's marked per-item.
- **Forget is soft.** There's no hard-delete endpoint over the network. Hard delete is a CLI admin operation with `--confirm` (axiom 6).
- **`follow_ups` is a small, cheap heuristic (not a planning step).** It returns 2-3 expand-query suggestions based on the activation frontier. Agents that don't want them can ignore them.

**Success metrics.**

- A fixture agent using only `/v1/memory/*` can maintain coherent multi-session state across 10 turns spanning 3 topics (qualitatively scored against a baseline of no memory).
- Token-budget enforcement: response token count within 5% of requested for budgets 500, 2k, 8k.
- p95 recall latency: <300ms local, <800ms federated 3-node, on 50k-article corpus.
- Federation round-trip: a recall with `federate=true` against 2 peers returns merged results with provenance per node.
- All endpoints pass auth + allowlist tests; no auth bypass.

**Out of scope.** Multi-modal observation beyond text (images/audio belong to Animus, not KNL — see project memory). Long-running observe pipelines (>30s); those are background jobs, not API responses. UI changes (this is the API layer; the connect plugin / app can adopt at its own pace).

**Citations and governance mapping.** Du (arXiv 2603.07670) — read-side mechanisms + agent loop framing. Lin, Li, Chen (arXiv 2604.16548) defines a six-phase memory lifecycle — **Write, Store, Retrieve, Execute, Share, Forget/Rollback** — cross-tabulated against four security objectives (integrity, confidentiality, availability, governance). P9's API maps to this lifecycle explicitly:

| Lifecycle phase | KNL surface |
|---|---|
| Write | `POST /v1/memory/observe` + RSA-JWT auth + dedup (P3) + idempotency key |
| Store | server-side; tier-aware (P8); audit-logged |
| Retrieve | `POST /v1/memory/recall` + auth + token-budget enforcement |
| Execute | **out of scope** — KNL is the memory layer, not the agent runtime |
| Share | `federate=true` opt-in per query; peers can refuse; K2K RSA-JWT |
| Forget/Rollback | `POST /v1/memory/forget` (soft-archive only); hard delete is CLI admin |

**Coverage of Lin et al.'s nine governance primitives.** This is the explicit matrix the Lin survey argues no published architecture covers in full. KNL's status, post-P9:

| # | Primitive | KNL status (post-P9) | Phase |
|---|---|---|---|
| 1 | Write-gate validation (pre-consolidation checks before persistence) | **Partial → Full.** P3 dedup is partial; P9 adds explicit pre-consolidation provenance + trust check. **Shared blind spot per survey — KNL can be a first-mover here.** | P3, P9 |
| 2 | Provenance tagging (source metadata on every entry) | **Full.** P3 `source_type` + P5 `extraction_method` + `confidence` + Lineage in P9 observe. | P3, P5, P9 |
| 3 | Versioning / snapshots | **Partial.** P8 audit log records modifications; full snapshot/diff store is a follow-up. Honest gap. | P8 (partial) |
| 4 | Compression audit (oversight of summarization that could amplify toxins) | **Full.** P7 reflections record source confidence + can't exceed it; reflection generation is logged. Addresses the "compression-amplified toxins" threat directly. | P7, P8 |
| 5 | Principal-scoped retrieval (access control distinguishing which users/agents see which memories) | **Partial.** K2K allowlist + RSA-JWT exist; per-memory scopes are a planned extension on top of P9. | P9 + follow-up |
| 6 | Audit trail (immutable log of read/write/modify) | **Full.** P8 audit log; append-only; K2K-replicable. | P8 |
| 7 | Post-deletion verification (confirm forgetting succeeded across all substrates) | **Full.** P9 `forget` verifies removal across LanceDB + SurrealDB. **Shared blind spot per survey — KNL can be a first-mover here too.** | P9 |
| 8 | Cross-substrate deletion protocol (synchronized removal) | **Full.** KNL's two-substrate architecture (LanceDB vectors + SurrealDB metadata/graph) requires this; P9 forget is the synchronization point. | P9 |
| 9 | Conflict detection and annealing (identify high-confidence errors, calibrate) | **Partial.** P3 dedup queue is the first cut; P9 surfaces conflicting retrievals; full confidence annealing is a follow-up. | P3, P9 + follow-up |

**Differentiation opportunity.** The survey states: *"write-gate validation and post-deletion verification are shared blind spots across every system examined."* KNL's local-first architecture + two-substrate design + soft-archive-by-default makes both achievable without architecture rewrites. Targeting full coverage of all nine is a credible product claim grounded in the public literature.

**Threat-model coverage from the survey, mapped to phases:**
- *Compression-Amplified Toxins* (reflections amplifying poisoned entries) → P7 reflection records source confidence; reflection score floor is min(source confidences), not max.
- *Cross-Agent Propagation / Social Contagion* → P9 `federate=true` is opt-in; peers can refuse; cross-node memory always tagged with origin node + provenance chain.
- *Black-Box Memory Extraction* → existing RSA-JWT + allowlist; P9 enforces principal-scoped retrieval before bundle construction.
- *Incomplete Deletion / Unlearning Confinement* → primitive #7 above; explicit cross-substrate verification.
- *Denial-of-Retrieval (corpus flooding)* → P3 dedup + P8 tier-aware reranker prevent low-salience flood from drowning legitimate memories.
- *Progressive Erosion / Benign Persistence* → P8 audit log + scheduled review CLI surfaces drift; user-pin overrides salience corruption.

---

## Deferred: P10+ — Policy-Learned Memory Control

The Mar 2026 survey identifies "policy-learned management" as the frontier: learning when to write, when to consolidate, when to demote, when to recall what depth, based on observed traces. We're not ready for this because we have no traces. P9's audit log + recall-result log *is* the dataset that makes this possible later. So this is a "two phases from now" thing, not a "next year" thing — it unlocks as soon as we have a few months of real usage logged.

When we get there, candidates: contextual bandits on activation-decay parameters (P6), learned tier-demotion thresholds (P8), learned reflection-cluster boundaries (P7). Each is parameterizable today; the trait abstractions in axiom 4 make swapping in a learned policy straightforward.

---

## Side-Track: Embedding Model Upgrade

`all-MiniLM-L6-v2` (384-dim) is the 2021 baseline. Stronger local-friendly options exist as of 2026:

- `nomic-embed-text` (768-dim, native Ollama, top of MTEB for small models)
- `BGE-small-en-v1.5` (384-dim, stronger than MiniLM at same size)
- `gte-small` (384-dim, comparable to BGE)

This is a side-track because it doesn't block other phases — they all work with any embedding. Best evaluated as an A/B behind a config flag, using the existing P3 dedup + P4 retrieval as the integration tests. If we change dimensions (384 → 768), it's a full re-index — sequence after P8 so the re-index runs once over the post-decay corpus, not the pre-decay one.

---

## Self-Review (against the goal stated up top)

Spec coverage: every frontier capability from the gap analysis maps to a phase (P5 multi-graph, P6 activation, P7 events+reflection, P8 decay, P9 API). Governance is preserved throughout. Local-first / Ollama-only / federation / pluggability axioms are each named as constraints in every phase that touches them.

Placeholder scan: each phase names exact files (creates + modifies), schema additions, success metrics, and out-of-scope boundaries. The "Files" tables are scoped enough to spawn a bite-sized implementation plan without re-deciding architecture.

Type consistency: `Event` (P7), `Article.tier` (P8), `RecallRequest` fields (P9), and edge structs (P5) are used consistently across the phases that touch them. `MemoryId` and `entity_id`/article id semantics inherit from P3.

Risk surface: LLM-dependent steps (P5 backfill, P6-via-init isn't LLM-dependent, P7 segmentation+reflection, P8 compaction-via-P7) all have explicit fallback paths or are opt-in. Migrations are versioned and tested. Performance budget is named (p95 ≤ 300ms local).

---

## Next Step

When this roadmap is accepted, the next document to produce is the detailed bite-sized implementation plan for **P5 — Decoupled Multi-Graph**, following the format of `2026-04-17-p3-entity-dedup-graph.md` and `2026-04-18-p4-graph-powered-retrieval.md` (Task N → bite-sized Steps with TDD checkboxes). That plan implements P5 and only P5; subsequent phases get their own dedicated plans when scheduled.

Per the project's git workflow (`CLAUDE.md`): each phase's plan is implemented on a feature branch (`feat/p5-multi-graph`, etc.), squash-merged via PR, never directly to main.
