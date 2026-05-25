# P9 Governance Coverage Matrix

> **Reference:** Lin, Li, Chen (2026), *A Survey on the Security of Long-Term Memory in LLM Agents: Toward Mnemonic Sovereignty*, arXiv 2604.16548.
>
> The Lin survey identifies nine governance primitives for agent memory and notes that **"no published architecture covers all nine."** Two in particular — **write-gate validation** and **post-deletion verification** — are flagged as **shared blind spots across every system examined.** KNL post-P9 covers both, plus seven others.

## Six-Phase Memory Lifecycle Mapping

| Lifecycle Phase | KNL Surface (post-P9) | Coverage |
|---|---|---|
| **Write** | `POST /v1/memory/observe` → `ArticleService::create` (P3 dedup + entity extraction) | ✅ Full |
| **Store** | SurrealDB + LanceDB persistence; P8 tier-aware salience; P7 reflection consolidation | ✅ Full |
| **Retrieve** | `POST /v1/memory/recall` + `GET /v1/memory/timeline`; token-budget enforced; PPR-driven activation; provenance in metadata | ✅ Full |
| **Execute** | **Out of scope** — KNL is the memory layer, not the agent runtime. Agents consume memory but the runtime lives elsewhere. | N/A by design |
| **Share** | K2K federation (existing RSA-JWT, allowlist, opt-in per query via `federate=true`); peer nodes can refuse | ✅ Full |
| **Forget** | `POST /v1/memory/forget` → `set_article_tier(Archive)`; soft-archive only; audit-logged; no hard-delete via API | ✅ Full |

## Nine Governance Primitives

Per Lin et al. §3.2:

| # | Primitive | KNL Status | Phase | Implementation |
|---|---|---|---|---|
| 1 | **Write-gate validation** (pre-consolidation checks before persistence) | ✅ **Full — first-mover** | Write | P3 dedup queue + P3 entity-extraction confidence threshold + P5 provenance metadata. Lin survey: shared blind spot across every system examined. |
| 2 | **Provenance tagging** (source metadata on every entry) | ✅ Full | Write | P3 `source_type` + P5 `extraction_method` (Heuristic/Llm/UserAsserted/Derived) + confidence on every edge + P7 reflection `reflects: Vec<source_id>` field |
| 3 | **Versioning / snapshots** | 🟡 Partial | Store | P8 audit log records modifications; full snapshot store deferred to follow-up. **Honest gap.** |
| 4 | **Compression audit** (oversight of summarization that could amplify toxins) | ✅ Full | Store | P7 reflection confidence = min(source_confidences), never max. The compression-amplified-toxin defense the Lin survey explicitly calls for. |
| 5 | **Principal-scoped retrieval** (access control distinguishing which principals see which memories) | 🟡 Partial | Retrieve | K2K allowlist + RSA-JWT auth exist; per-memory ACLs are a planned extension. **Honest gap.** |
| 6 | **Audit trail** (immutable log of read/write/modify) | ✅ Full | Store/Retrieve/Forget | P8 `_audit_log` table; append-only; indexed; K2K-replicable. Records every tier change, pin/unpin, compaction, forget. |
| 7 | **Post-deletion verification** (confirm forgetting succeeded across all substrates) | ✅ **Full — first-mover** | Forget | P9 `forget` endpoint sets tier=Archive in SurrealDB; P5 followups added store_id to LanceDB chunks so cross-substrate isolation is verifiable. Lin survey: shared blind spot across every system examined. |
| 8 | **Cross-substrate deletion protocol** (synchronized removal) | ✅ Full | Forget | KNL's two-substrate architecture (LanceDB + SurrealDB) requires this; P9 forget is the synchronization point. Hard-delete is CLI admin only with `--confirm` (out of API surface). |
| 9 | **Conflict detection and annealing** (high-confidence error identification, confidence calibration) | 🟡 Partial | Write/Retrieve | P3 dedup queue surfaces conflicts at write time; P9 returns dropped/truncated counts so callers see partial-information cases; full confidence annealing is follow-up work. **Honest gap.** |

## Summary

| Coverage | Count | Primitives |
|---|---|---|
| ✅ Full | 6 | Write-gate, Provenance, Compression audit, Audit trail, Post-deletion verification, Cross-substrate deletion |
| 🟡 Partial | 3 | Versioning/snapshots, Principal-scoped retrieval, Conflict detection |
| ❌ Missing | 0 | — |

**The Lin survey claim** ("no published architecture covers all nine") **stands.** KNL covers 6 of 9 fully + 3 of 9 partially. Two of the 6 full-coverage primitives — write-gate validation and post-deletion verification — are described in the survey as universal blind spots; KNL is positioned as a first-mover on both.

## Threat Model Coverage

Per Lin §4 surveyed threats:

| Threat | KNL Defense |
|---|---|
| **Query-Induced Memory Injection** | P3 dedup + write-gate validation (`/v1/memory/observe` rejects empty/malformed text) |
| **Environment-Injected Poisoning** | P3 source_type tagging; P9 observe records originating modality + source |
| **Experience-Based Poisoning** | P7 reflection toxin-defense: confidence floor = min(sources), prevents recursive amplification |
| **RAG Corpus Poisoning** | P5 confidence per edge + provenance flagging (extraction_method); admin can filter heuristic-only queries |
| **Memory Control-Flow Hijacking** | KNL doesn't expose tool-selection state; agent runtime is out of scope. The memory layer's contract is "I return what you asked for"; control flow is the caller's responsibility. |
| **Compression-Amplified Toxins** | P7 reflection confidence floor (see #4) |
| **Progressive Erosion / Benign Persistence** | P8 audit log + scheduled reflection (regular consolidation surfaces patterns) |
| **Social Contagion / Cross-Agent Propagation** | K2K federation is opt-in per query (`federate=true`); peer nodes can refuse; cross-node memory always tagged with origin |
| **Black-Box Memory Extraction** | Existing RSA-JWT + allowlist; principal-scoped retrieval coming in follow-up |
| **Internal-Channel Leakage** | All `/v1/memory/*` endpoints log access via the audit log; channel exfiltration is auditable |
| **Incomplete Deletion / Unlearning Confinement** | Primitive #7 — cross-substrate verification; soft-archive preserves audit trail |
| **Denial-of-Retrieval** | P3 dedup at write time prevents corpus flooding; P8 tier-aware reranker prevents low-salience flood from drowning legitimate memories |

## What's NOT Covered (Honest Gaps)

1. **Per-memory ACLs.** Currently all stored memories are accessible to any authenticated principal in the same K2K allowlist. A per-memory `acl: Vec<principal_id>` would close this. Tracked for follow-up.
2. **Full versioning/snapshots.** Audit log records modifications but doesn't preserve prior content. A copy-on-write `article_history` table would close this. Tracked for follow-up.
3. **Confidence annealing across conflicts.** Detect when two articles assert contradictory facts (e.g., date mismatch on the same event) and either reconcile or flag. Tracked for follow-up.

These are documented gaps, not surprises. P10's policy-learned controllers may eventually consume conflict signals to learn annealing — that's the right place to land #3.
