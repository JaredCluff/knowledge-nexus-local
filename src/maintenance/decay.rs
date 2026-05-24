//! Salience computation + tier assignment (P8).
//!
//! Pure functions; no DB I/O. The four formula implementations are from
//! peer-reviewed sources; each computes a salience value in [0.0, 1.0]
//! given an article's metadata and the current time.

use chrono::{DateTime, Utc};

use crate::config::{DecayConfig, SalienceFormula};
use crate::store::Tier;

/// Inputs to the salience function. Implementations vary in which fields
/// they use; the struct collects everything any formula might need.
pub struct SalienceInput<'a> {
    pub importance_score: f64,
    pub last_accessed_at: &'a str,
    pub created_at: &'a str,
    pub access_count: i64,
    /// Optional query-relevance score for Generative Agents formula.
    /// Pass 0.0 if not in a query context.
    pub relevance: f64,
}

/// Compute salience for the given input under the configured formula.
/// Output is clamped to [0.0, 1.0].
pub fn salience(input: &SalienceInput<'_>, config: &DecayConfig, now: DateTime<Utc>) -> f64 {
    let raw = match config.formula {
        SalienceFormula::ActivationDriven => salience_activation(input, config, now),
        SalienceFormula::MemoryOsHeat => salience_memory_os(input, config, now),
        SalienceFormula::GenerativeAgents => salience_generative_agents(input, config, now),
        SalienceFormula::Ebbinghaus => salience_ebbinghaus(input, config, now),
    };
    raw.clamp(0.0, 1.0)
}

/// Determine tier from a salience value + thresholds.
pub fn tier_for_salience(s: f64, config: &DecayConfig) -> Tier {
    if s >= config.hot_threshold {
        Tier::Hot
    } else if s >= config.warm_threshold {
        Tier::Warm
    } else if s >= config.cold_threshold {
        Tier::Cold
    } else {
        Tier::Archive
    }
}

fn days_since(timestamp_rfc3339: &str, now: DateTime<Utc>) -> f64 {
    match DateTime::parse_from_rfc3339(timestamp_rfc3339) {
        Ok(t) => {
            let dt: DateTime<Utc> = t.with_timezone(&Utc);
            let delta = now - dt;
            delta.num_seconds() as f64 / 86_400.0
        }
        Err(_) => 0.0, // unparseable → treat as fresh
    }
}

/// SYNAPSE-aligned (default): `salience = importance · exp(-λ · days_since_access)`.
fn salience_activation(input: &SalienceInput<'_>, config: &DecayConfig, now: DateTime<Utc>) -> f64 {
    let ts = if input.last_accessed_at.is_empty() {
        input.created_at
    } else {
        input.last_accessed_at
    };
    let days = days_since(ts, now).max(0.0);
    input.importance_score * (-config.lambda * days).exp()
}

/// MemoryOS heat formula (arXiv 2506.06326).
/// `heat = α·visits + β·interaction_length + γ·recency_factor`
/// where recency_factor = exp(-Δseconds / μ).
///
/// Output is normalized to ~[0.0, 1.0] via sigmoid squash so it composes
/// with the same thresholds as the other formulas. `interaction_length`
/// is approximated by access_count (we don't track per-access duration).
fn salience_memory_os(input: &SalienceInput<'_>, config: &DecayConfig, now: DateTime<Utc>) -> f64 {
    let ts = if input.last_accessed_at.is_empty() {
        input.created_at
    } else {
        input.last_accessed_at
    };
    let recency_factor = match DateTime::parse_from_rfc3339(ts) {
        Ok(t) => {
            let secs = (now - t.with_timezone(&Utc)).num_seconds() as f64;
            (-secs / config.heat_mu).exp()
        }
        Err(_) => 1.0,
    };
    let visits = input.access_count as f64;
    let interaction = input.access_count as f64; // approximation
    let heat = config.heat_alpha * visits
        + config.heat_beta * interaction
        + config.heat_gamma * recency_factor;
    // Sigmoid squash to keep the output composable with tier thresholds.
    1.0 / (1.0 + (-heat).exp())
}

/// Generative Agents formula (Park et al.):
/// `salience = w_rec·recency + w_rel·relevance + w_imp·importance`
/// where recency = ga_decay^days_since_access.
fn salience_generative_agents(input: &SalienceInput<'_>, config: &DecayConfig, now: DateTime<Utc>) -> f64 {
    let ts = if input.last_accessed_at.is_empty() {
        input.created_at
    } else {
        input.last_accessed_at
    };
    let days = days_since(ts, now).max(0.0);
    let recency = config.ga_decay.powf(days);
    config.ga_w_recency * recency
        + config.ga_w_relevance * input.relevance
        + config.ga_w_importance * input.importance_score
}

/// MemoryBank Ebbinghaus-curve: `salience = exp(-t/S)` where S grows with
/// access_count (reinforcement). t is days since last access.
fn salience_ebbinghaus(input: &SalienceInput<'_>, config: &DecayConfig, now: DateTime<Utc>) -> f64 {
    let ts = if input.last_accessed_at.is_empty() {
        input.created_at
    } else {
        input.last_accessed_at
    };
    let days = days_since(ts, now).max(0.0);
    // Reinforcement: S grows as access_count grows (sqrt for diminishing returns)
    let reinforced_strength =
        config.ebbinghaus_strength * (1.0 + (input.access_count as f64).sqrt());
    if reinforced_strength <= 0.0 {
        return 0.0;
    }
    (-days / reinforced_strength).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn epoch_2026() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-05-24T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn input_with(last_accessed_at: &'static str, importance: f64, access_count: i64) -> SalienceInput<'static> {
        SalienceInput {
            importance_score: importance,
            last_accessed_at,
            created_at: last_accessed_at, // assume created == last accessed for these tests
            access_count,
            relevance: 0.0,
        }
    }

    #[test]
    fn activation_driven_decays_over_time() {
        let cfg = DecayConfig::default();
        let now = epoch_2026();

        // Just accessed: ~importance
        let fresh = salience(&input_with("2026-05-24T00:00:00Z", 0.8, 1), &cfg, now);
        assert!((fresh - 0.8).abs() < 1e-6);

        // 35 days ago (half-life with λ=0.02): ~importance / 2
        let half_life = salience(&input_with("2026-04-19T00:00:00Z", 0.8, 1), &cfg, now);
        assert!(half_life > 0.39 && half_life < 0.41,
            "expected ~0.4 at λ=0.02 × 35 days; got {}", half_life);

        // 365 days ago: nearly 0
        let year_old = salience(&input_with("2025-05-24T00:00:00Z", 0.8, 1), &cfg, now);
        assert!(year_old < 0.001, "expected near-zero at 365 days; got {}", year_old);
    }

    #[test]
    fn activation_driven_respects_importance() {
        let cfg = DecayConfig::default();
        let now = epoch_2026();
        let low = salience(&input_with("2026-05-24T00:00:00Z", 0.2, 1), &cfg, now);
        let high = salience(&input_with("2026-05-24T00:00:00Z", 0.9, 1), &cfg, now);
        assert!(high > low);
    }

    #[test]
    fn tier_for_salience_uses_thresholds() {
        let cfg = DecayConfig::default();
        assert_eq!(tier_for_salience(0.9, &cfg), Tier::Hot);
        assert_eq!(tier_for_salience(0.3, &cfg), Tier::Warm);
        assert_eq!(tier_for_salience(0.05, &cfg), Tier::Cold);
        assert_eq!(tier_for_salience(0.005, &cfg), Tier::Archive);
    }

    #[test]
    fn memory_os_heat_increases_with_visits() {
        let mut cfg = DecayConfig::default();
        cfg.formula = SalienceFormula::MemoryOsHeat;
        let now = epoch_2026();
        let low_visits = salience(&input_with("2026-05-24T00:00:00Z", 0.5, 1), &cfg, now);
        let high_visits = salience(&input_with("2026-05-24T00:00:00Z", 0.5, 100), &cfg, now);
        assert!(high_visits > low_visits,
            "more visits should yield higher heat; got {} vs {}", low_visits, high_visits);
    }

    #[test]
    fn memory_os_heat_clamped_to_unit_interval() {
        let mut cfg = DecayConfig::default();
        cfg.formula = SalienceFormula::MemoryOsHeat;
        let now = epoch_2026();
        let s = salience(&input_with("2026-05-24T00:00:00Z", 0.5, 1_000_000), &cfg, now);
        assert!(s >= 0.0 && s <= 1.0, "value out of range: {}", s);
    }

    #[test]
    fn generative_agents_weights_recency_relevance_importance() {
        let mut cfg = DecayConfig::default();
        cfg.formula = SalienceFormula::GenerativeAgents;
        let now = epoch_2026();
        // Fresh access + high importance + zero relevance: ga_w_recency·1 + 0 + ga_w_importance·1
        let s = salience(&input_with("2026-05-24T00:00:00Z", 1.0, 1), &cfg, now);
        let expected = cfg.ga_w_recency + cfg.ga_w_importance;  // recency=1, relevance=0, importance=1
        assert!((s - expected.min(1.0)).abs() < 1e-6,
            "expected {} (clamped), got {}", expected.min(1.0), s);
    }

    #[test]
    fn ebbinghaus_decays_with_time_and_strengthens_with_access() {
        let mut cfg = DecayConfig::default();
        cfg.formula = SalienceFormula::Ebbinghaus;
        let now = epoch_2026();

        // 7 days ago, accessed once
        let single = salience(&input_with("2026-05-17T00:00:00Z", 0.5, 1), &cfg, now);
        // 7 days ago, accessed many times → higher strength → slower decay
        let many = salience(&input_with("2026-05-17T00:00:00Z", 0.5, 100), &cfg, now);
        assert!(many > single,
            "reinforced (100 accesses) should decay slower; got {} vs {}", many, single);
    }

    #[test]
    fn salience_clamps_to_unit_interval_for_all_formulas() {
        let now = epoch_2026();
        for formula in [
            SalienceFormula::ActivationDriven,
            SalienceFormula::MemoryOsHeat,
            SalienceFormula::GenerativeAgents,
            SalienceFormula::Ebbinghaus,
        ] {
            let mut cfg = DecayConfig::default();
            cfg.formula = formula;
            let s = salience(&input_with("2026-05-24T00:00:00Z", 0.5, 10), &cfg, now);
            assert!(s >= 0.0 && s <= 1.0,
                "{:?} produced out-of-range salience: {}", formula, s);
        }
    }
}
