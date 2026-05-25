# P10 Policy-Learned Memory Control — Scaffolding

> **Status:** Scaffolding only. P10 lands the trait abstractions, default rule-based implementations, and trace-logging infrastructure for future learned policies. **No policies are trained in P10.**

## Why scaffolding

Per Du survey §9.9 (arXiv 2603.07670) and Luo et al. Experience stage (arXiv 2605.06716), policy-learned memory control is the next frontier — but it requires training trajectories that don't exist yet. P9's audit log + P10's `_policy_traces` table together are the training-data substrate. Offline training becomes possible once 3-6 months of real usage accumulate.

## What P10 ships

### Three policy traits (`src/policy/`)

| Trait | Decision | Default Impl |
|---|---|---|
| `DecayPolicy` | salience(article) → f64 | `DefaultDecayPolicy` (delegates to P8 SYNAPSE-aligned formula) |
| `ReflectionTriggerPolicy` | should_reflect(store, ingest_count, last_reflection_at) → bool | `DefaultReflectionTrigger` (100-ingest threshold + 6-hour cooldown) |
| `ActivationWeightPolicy` | intent_weights(query, intent) → IntentWeights | `DefaultActivationWeights` (delegates to P6 MAGMA Table 6) |

`PolicyRegistry::with_defaults(decay_config)` constructs all three. The registry is Arc-friendly and can be shared across callsites.

### Trace logging (`_policy_traces` table)

Each policy decision can be logged with:
- `policy_name` — which impl made the decision
- `decision_type` — decay / reflection_trigger / activation_weight
- `input_features` — JSON (FLEXIBLE) — features the policy observed
- `action` — JSON (FLEXIBLE) — the chosen action
- `outcome` — JSON (FLEXIBLE), optional — measured result (filled later)
- `recorded_at` — RFC3339

`PolicyConfig.trace_sampling_rate` (default 1.0) controls how often traces are written. Lower in production to manage volume.

### CLI

```bash
cargo run -- policy-traces [--store <id>] [--policy <name>] [--since <rfc3339>] [-l N]
```

Dumps traces for offline analysis. Pipe to `jq` or import into a training pipeline.

## How to add a learned policy later

Suppose you've trained a model that predicts salience given (importance, access_count, days_since_access). To plug it in:

1. Create `src/policy/learned_decay.rs`:
   ```rust
   pub struct LearnedDecayPolicy { model: MyModel }

   impl DecayPolicy for LearnedDecayPolicy {
       fn salience(&self, input: &SalienceInput, now: DateTime<Utc>) -> f64 {
           self.model.predict(...)
       }
       fn name(&self) -> &str { "learned_v1" }
   }
   ```

2. Update `PolicyRegistry::new(config)` (currently `with_defaults`) to dispatch by `config.policy.decay_policy_name`:
   ```rust
   pub fn new(config: &Config) -> Self {
       let decay: Arc<dyn DecayPolicy> = match config.policy.decay_policy_name.as_str() {
           "learned_v1" => Arc::new(LearnedDecayPolicy::load(...)),
           _ => Arc::new(DefaultDecayPolicy::new(config.decay.clone())),
       };
       // ...same for reflection and activation
   }
   ```

3. Set `policy.decay_policy_name = "learned_v1"` in config.

4. Traces continue to log under the new policy name — the training loop closes.

No callsite changes are needed.

## Training-data substrate

`_policy_traces` + `_audit_log` together capture:
- Every tier transition (what we did + why)
- Every recall query (what intent classified + which results surfaced + which were clicked through implicitly via P8 access tracking)
- Every reflection trigger (when fired + how many sources)

This is sufficient for offline batch training of:
- Decay-formula constants per-corpus (currently λ=0.02 is hand-tuned; could be learned per-user)
- Reflection-trigger thresholds (currently 100/6h is hand-tuned)
- Intent-weight table (currently static MAGMA Table 6; could be learned per-corpus)

## Deferred (P11+)

- Online learning / RL — requires deployment-time policy updates. Out of scope.
- Trace-outcome closure for activation queries (we log the decision but not "did the user find what they wanted"). Requires explicit success signals from the agent.
- Per-user vs per-corpus policies. Currently one policy per process.
