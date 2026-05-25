//! Default rule-based DecayPolicy. Delegates to P8 maintenance::decay::salience.

use chrono::{DateTime, Utc};

use crate::config::DecayConfig;
use crate::maintenance::decay::{salience as compute_salience, SalienceInput};
use crate::policy::DecayPolicy;

pub struct DefaultDecayPolicy {
    config: DecayConfig,
}

impl DefaultDecayPolicy {
    pub fn new(config: DecayConfig) -> Self {
        Self { config }
    }
}

impl DecayPolicy for DefaultDecayPolicy {
    fn salience(&self, input: &SalienceInput<'_>, now: DateTime<Utc>) -> f64 {
        compute_salience(input, &self.config, now)
    }

    fn name(&self) -> &str {
        "default_synapse_aligned"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_decay_returns_synapse_aligned_value() {
        let policy = DefaultDecayPolicy::new(DecayConfig::default());
        let now = DateTime::parse_from_rfc3339("2026-05-24T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        // Fresh article should retain near-full salience
        let input = SalienceInput {
            importance_score: 0.8,
            last_accessed_at: "2026-05-24T00:00:00Z",
            created_at: "2026-05-24T00:00:00Z",
            access_count: 1,
            relevance: 0.0,
        };
        let s = policy.salience(&input, now);
        assert!(s > 0.79, "fresh article should retain ~0.8 salience; got {}", s);
    }

    #[test]
    fn default_decay_name_is_stable() {
        let policy = DefaultDecayPolicy::new(DecayConfig::default());
        assert_eq!(policy.name(), "default_synapse_aligned");
    }
}
