//! Default rule-based ReflectionTriggerPolicy: threshold + cooldown.

use chrono::{DateTime, Utc};

use crate::policy::ReflectionTriggerPolicy;

pub struct DefaultReflectionTrigger {
    ingest_threshold: usize,
    min_interval_hours: u64,
}

impl DefaultReflectionTrigger {
    pub fn new(ingest_threshold: usize, min_interval_hours: u64) -> Self {
        Self {
            ingest_threshold,
            min_interval_hours,
        }
    }
}

impl ReflectionTriggerPolicy for DefaultReflectionTrigger {
    fn should_reflect(
        &self,
        _store_id: &str,
        ingest_count: usize,
        last_reflection_at: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> bool {
        if ingest_count < self.ingest_threshold {
            return false;
        }
        match last_reflection_at {
            None => true,
            Some(last) => {
                let elapsed = now - last;
                elapsed.num_hours() as u64 >= self.min_interval_hours
            }
        }
    }

    fn name(&self) -> &str {
        "default_rate_threshold"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(rfc: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn threshold_not_met_returns_false() {
        let policy = DefaultReflectionTrigger::new(100, 6);
        let now = t("2026-05-24T00:00:00Z");
        assert!(!policy.should_reflect("s1", 50, None, now));
    }

    #[test]
    fn threshold_met_first_time_returns_true() {
        let policy = DefaultReflectionTrigger::new(100, 6);
        let now = t("2026-05-24T00:00:00Z");
        assert!(policy.should_reflect("s1", 100, None, now));
        assert!(policy.should_reflect("s1", 200, None, now));
    }

    #[test]
    fn threshold_met_but_within_cooldown_returns_false() {
        let policy = DefaultReflectionTrigger::new(100, 6);
        let last = t("2026-05-23T20:00:00Z"); // 4 hours ago
        let now = t("2026-05-24T00:00:00Z");
        assert!(
            !policy.should_reflect("s1", 200, Some(last), now),
            "should be in cooldown"
        );

        // 7 hours ago — past cooldown
        let last_old = t("2026-05-23T17:00:00Z");
        assert!(policy.should_reflect("s1", 200, Some(last_old), now));
    }
}
