# P10: Policy-Learned Memory Control — Scaffolding Plan

> **REQUIRED SUB-SKILL:** superpowers:subagent-driven-development
>
> **Prerequisites:** P4-P9 merged or stacked. Branches off `feat/p9-agent-api`.

**Goal:** Build the trait abstractions, default rule-based implementations, and trace-logging infrastructure that make P10's full learned-policy work possible later. This phase is SCAFFOLDING ONLY — we do not train any policies in this PR. The roadmap explicitly defers learned controllers because no usage traces exist yet. P9's audit log + P10's new trace tables ARE the dataset that unlocks learned policies later.

**Architecture in one paragraph.** Three new traits — `DecayPolicy`, `ReflectionTriggerPolicy`, `ActivationWeightPolicy` — each with a default rule-based implementation that mirrors today's hardcoded behavior (SYNAPSE decay constants, ingest-counter threshold, MAGMA intent table). A new `_policy_traces` SurrealDB table captures every policy decision with input features + chosen action + outcome (where measurable). A `PolicyRegistry` resolves trait impls by name at startup so swapping in a learned policy is a config-file change, not a code change. Trait methods are sync where possible to keep them swappable for inference; async only where the policy genuinely needs DB access.

**Why scaffolding now.** Per the roadmap and Du survey (arXiv 2603.07670 §9.9), policy-learned controllers are the next frontier but require training trajectories that don't exist yet. P9's audit log is the first half of the training data; P10 adds the second half — decision-level traces that capture WHICH policy fired, WHAT input features it saw, and WHAT outcome resulted. Once 3-6 months of real usage accumulate, the data supports offline policy training.

**Tech Stack:** Existing — Rust, SurrealDB. No ML deps yet (we're not training, just collecting).

## Bibliography

- Du (arXiv 2603.07670, §9.9 Future Direction "Policy-Learned Memory Management")
- Luo et al. (arXiv 2605.06716) — Storage → Reflection → Experience progression; Experience is the policy-learned frontier
- MAGMA (arXiv 2601.03236) — already uses query-adaptive policies (rule-based); learning the weight table would be P10's first concrete win
- SYNAPSE (arXiv 2601.02744) — decay constants are hand-tuned in their paper; same constants in KNL today; learning per-corpus optima is P10's second concrete win

## File Structure

| Action | Path | Responsibility |
|---|---|---|
| Create | `src/policy/mod.rs` | Module root: trait definitions + PolicyRegistry |
| Create | `src/policy/decay.rs` | `DecayPolicy` trait + default rule-based impl |
| Create | `src/policy/reflection.rs` | `ReflectionTriggerPolicy` trait + default impl |
| Create | `src/policy/activation.rs` | `ActivationWeightPolicy` trait + default impl |
| Create | `src/policy/traces.rs` | Trace logging: insert PolicyTrace records |
| Modify | `src/store/schema.rs` | `_policy_traces` table DDL; schema → `1.0.0-p10` |
| Modify | `src/store/models.rs` | `PolicyTrace` struct |
| Modify | `src/store/mod.rs` | `write_policy_trace`, `list_policy_traces` |
| Modify | `src/lib.rs` | `pub mod policy;` |
| Modify | `src/config/mod.rs` | `PolicyConfig` section with policy-name overrides + trace-sampling rate |
| Modify | `src/maintenance/decay.rs` | Dispatch through `DecayPolicy` trait when policy is configured |

---

### Task 1: Trait definitions + PolicyRegistry

**Files:** `src/policy/mod.rs`, `src/lib.rs`

- [ ] `pub trait DecayPolicy: Send + Sync { fn salience(&self, input: &SalienceInput, now: DateTime<Utc>) -> f64; fn name(&self) -> &str; }` (sync — called per-article in tier transitions; performance critical)
- [ ] `pub trait ReflectionTriggerPolicy: Send + Sync { fn should_reflect(&self, store_id: &str, ingest_count: usize, last_reflection_at: Option<DateTime<Utc>>) -> bool; fn name(&self) -> &str; }`
- [ ] `pub trait ActivationWeightPolicy: Send + Sync { fn intent_weights(&self, query: &str, intent: Intent) -> IntentWeights; fn name(&self) -> &str; }`
- [ ] `PolicyRegistry` holding `Arc<dyn DecayPolicy>`, `Arc<dyn ReflectionTriggerPolicy>`, `Arc<dyn ActivationWeightPolicy>`. Constructor reads `PolicyConfig` and looks up impls by name (default → rule-based).
- [ ] 3 tests covering trait registration + default resolution.

```bash
git commit -m "feat(p10): policy trait abstractions + PolicyRegistry"
```

---

### Task 2: Default rule-based DecayPolicy

**Files:** `src/policy/decay.rs`

- [ ] `pub struct DefaultDecayPolicy { config: DecayConfig }` — delegates to the existing `crate::maintenance::decay::salience(...)` function from P8.
- [ ] `impl DecayPolicy for DefaultDecayPolicy` returns the SYNAPSE-aligned activation-driven default by default.
- [ ] `name() -> "default_synapse_aligned"`.
- [ ] 2 tests.

```bash
git commit -m "feat(p10): DefaultDecayPolicy delegating to P8 SYNAPSE-aligned salience"
```

---

### Task 3: Default rule-based ReflectionTriggerPolicy

**Files:** `src/policy/reflection.rs`

- [ ] `pub struct DefaultReflectionTrigger { ingest_threshold: usize, min_interval_hours: u64 }`.
- [ ] `should_reflect()` returns true when `ingest_count >= ingest_threshold` AND (`last_reflection_at` is None OR `now - last_reflection_at > min_interval`).
- [ ] `name() -> "default_rate_threshold"`.
- [ ] 3 tests covering: threshold-not-met, threshold-met-first-time, threshold-met-but-too-recent.

```bash
git commit -m "feat(p10): DefaultReflectionTrigger with threshold + cooldown"
```

---

### Task 4: Default rule-based ActivationWeightPolicy

**Files:** `src/policy/activation.rs`

- [ ] `pub struct DefaultActivationWeights { intent_classifier: ... }` — delegates to existing `crate::retrieval::intent::classify` + `Intent::weights()`.
- [ ] `intent_weights(query, intent)` ignores the query and returns the MAGMA-style table from Intent.
- [ ] `name() -> "default_magma_table"`.
- [ ] 2 tests.

```bash
git commit -m "feat(p10): DefaultActivationWeights delegating to P6 MAGMA intent table"
```

---

### Task 5: PolicyTrace schema + Store methods

**Files:** `src/store/schema.rs`, `src/store/models.rs`, `src/store/mod.rs`

- [ ] `PolicyTrace` struct: `id, store_id, policy_name, decision_type ("decay"|"reflection_trigger"|"activation_weight"), input_features (Value), action (Value), outcome (Option<Value>), recorded_at`.
- [ ] `_policy_traces` DDL: schema bump to `1.0.0-p10`; index on `policy_name + recorded_at` for offline-batch queries; FLEXIBLE on input_features/action/outcome (per the P8 audit-log lesson).
- [ ] `Store::write_policy_trace(trace)` + `Store::list_policy_traces(filter, limit)`.
- [ ] 3 tests.

```bash
git commit -m "feat(p10): _policy_traces table + trace CRUD"
```

---

### Task 6: Trace-logging hooks in callsites

**Files:** `src/maintenance/decay.rs`, others

- [ ] `nightly_tier_transition` records a PolicyTrace per article transition: input_features={tier, importance, days_since_access}, action={new_tier, salience}.
- [ ] Reflection trigger records when fired (input: ingest_count + last_reflection_at; action: triggered y/n).
- [ ] ActivationEngine records intent_weights chosen per recall query (input: query string, classified intent; action: weight vector).
- [ ] Sampled by `PolicyConfig.trace_sampling_rate` (default 1.0 = 100%; can be lowered for production volume).
- [ ] 2 tests verifying traces are written.

```bash
git commit -m "feat(p10): trace-logging hooks in decay + reflection + activation"
```

---

### Task 7: PolicyConfig + CLI + docs

**Files:** `src/config/mod.rs`, `src/main.rs`, `docs/p10-policy-scaffolding.md`

- [ ] `PolicyConfig { decay_policy_name, reflection_policy_name, activation_policy_name, trace_sampling_rate }` — all default to the rule-based impls.
- [ ] CLI `policy-traces [--since] [--policy] [--limit]` to dump traces for offline analysis.
- [ ] `docs/p10-policy-scaffolding.md` explains the architecture, the deferred learned-policy frontier, and how to plug in a learned impl later.

```bash
git commit -m "feat(p10): PolicyConfig + policy-traces CLI + scaffolding docs"
```

---

### Task 8: Push + open PR (base = `feat/p9-agent-api`)

```bash
git push -u origin feat/p10-policy-scaffolding
gh pr create --base feat/p9-agent-api --title "P10: Policy-Learned Memory Control — Scaffolding" --body "..."
```

---

## Self-Review Checklist

- 3 traits exist (DecayPolicy, ReflectionTriggerPolicy, ActivationWeightPolicy)
- Default rule-based impl for each, behaviorally identical to current hardcoded paths
- PolicyRegistry resolves trait impls by name from config
- `_policy_traces` table records every policy decision + features + outcome
- Trace sampling is configurable (default 1.0)
- CLI surfaces traces for offline analysis
- Docs explain how to swap in a learned policy later
- No learned policies trained in this PR (scope is scaffolding only)
- All tests pass; no new clippy warnings
