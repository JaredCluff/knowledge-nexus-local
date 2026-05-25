//! Default rule-based ActivationWeightPolicy: returns MAGMA static table
//! (P6 Task 3) regardless of query content. Future learned policies may
//! adapt weights per-query based on outcomes observed in policy_traces.

use crate::policy::ActivationWeightPolicy;
use crate::retrieval::intent::{Intent, IntentWeights};

pub struct DefaultActivationWeights;

impl DefaultActivationWeights {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DefaultActivationWeights {
    fn default() -> Self {
        Self::new()
    }
}

impl ActivationWeightPolicy for DefaultActivationWeights {
    fn intent_weights(&self, _query: &str, intent: Intent) -> IntentWeights {
        // Delegate to the static MAGMA table from P6 Task 3.
        intent.weights()
    }

    fn name(&self) -> &str {
        "default_magma_table"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn why_intent_boosts_causal_edge() {
        let policy = DefaultActivationWeights::new();
        let w = policy.intent_weights("why did X happen", Intent::Why);
        assert!(
            w.caused_by > 1.0,
            "Why intent should boost causal_by; got {}",
            w.caused_by
        );
    }

    #[test]
    fn open_domain_intent_uses_uniform_weights() {
        let policy = DefaultActivationWeights::new();
        let w = policy.intent_weights("anything", Intent::OpenDomain);
        assert_eq!(w.entity_overlap, 1.0);
        assert_eq!(w.caused_by, 1.0);
        assert_eq!(w.precedes, 1.0);
    }
}
