//! Memory control policies (P10 scaffolding).
//!
//! Three trait abstractions for the policy-learned memory control frontier
//! (Du survey arXiv 2603.07670 §9.9; Luo et al. arXiv 2605.06716 Experience stage):
//! - `DecayPolicy` — what salience to assign an article (drives tier transitions)
//! - `ReflectionTriggerPolicy` — when to fire reflection jobs
//! - `ActivationWeightPolicy` — per-intent edge-weight multipliers (MAGMA-style)
//!
//! P10 ships SCAFFOLDING ONLY. Each trait has a default rule-based impl that
//! mirrors the current hardcoded behavior. Learned implementations require
//! training data that P9's audit log + P10's policy_traces will accumulate;
//! offline training is the next concrete frontier.

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::config::DecayConfig;
use crate::maintenance::decay::SalienceInput;
use crate::retrieval::intent::{Intent, IntentWeights};

pub mod activation;
pub mod decay;
pub mod reflection;
pub mod traces;

pub use activation::DefaultActivationWeights;
pub use decay::DefaultDecayPolicy;
pub use reflection::DefaultReflectionTrigger;

// ----- Trait definitions -----

/// Computes salience for an article. Drives tier transitions and PPR
/// personalization weighting. Sync — called per-article in hot paths.
pub trait DecayPolicy: Send + Sync {
    fn salience(&self, input: &SalienceInput<'_>, now: DateTime<Utc>) -> f64;
    fn name(&self) -> &str;
}

/// Decides whether reflection should fire for a given store given its
/// recent activity. Returns true to trigger; false to wait. Sync —
/// called on every ingest.
pub trait ReflectionTriggerPolicy: Send + Sync {
    fn should_reflect(
        &self,
        store_id: &str,
        ingest_count: usize,
        last_reflection_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> bool;
    fn name(&self) -> &str;
}

/// Returns per-edge-type weight multipliers for the activation engine
/// given a query string and its classified intent. Default impl ignores
/// the query and returns the MAGMA static table.
pub trait ActivationWeightPolicy: Send + Sync {
    fn intent_weights(&self, query: &str, intent: Intent) -> IntentWeights;
    fn name(&self) -> &str;
}

// ----- Registry -----

/// Holds the active policy implementations for the current process.
/// Constructed once at startup from `PolicyConfig`; passed by Arc into
/// callsites that need policy dispatch.
pub struct PolicyRegistry {
    pub decay: Arc<dyn DecayPolicy>,
    pub reflection: Arc<dyn ReflectionTriggerPolicy>,
    pub activation: Arc<dyn ActivationWeightPolicy>,
}

impl PolicyRegistry {
    /// Construct a registry with all default rule-based policies.
    /// Future: read `PolicyConfig` and look up named impls — but for P10
    /// scaffolding, defaults are the only impl.
    pub fn with_defaults(decay_config: DecayConfig) -> Self {
        Self {
            decay: Arc::new(DefaultDecayPolicy::new(decay_config)),
            reflection: Arc::new(DefaultReflectionTrigger::new(100, 6)),
            activation: Arc::new(DefaultActivationWeights::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_with_defaults_has_named_policies() {
        let reg = PolicyRegistry::with_defaults(DecayConfig::default());
        assert_eq!(reg.decay.name(), "default_synapse_aligned");
        assert_eq!(reg.reflection.name(), "default_rate_threshold");
        assert_eq!(reg.activation.name(), "default_magma_table");
    }

    #[test]
    fn registry_policies_are_independent() {
        // Verify the Arc<dyn ...> wrappers don't tangle types
        let reg = PolicyRegistry::with_defaults(DecayConfig::default());
        let d1 = Arc::clone(&reg.decay);
        let d2 = Arc::clone(&reg.decay);
        assert!(Arc::ptr_eq(&d1, &d2));
    }

    #[test]
    fn registry_can_be_passed_by_arc() {
        // Verifies PolicyRegistry can itself be Arc-wrapped and shared
        let reg = Arc::new(PolicyRegistry::with_defaults(DecayConfig::default()));
        let r1 = Arc::clone(&reg);
        assert_eq!(r1.decay.name(), "default_synapse_aligned");
    }
}
