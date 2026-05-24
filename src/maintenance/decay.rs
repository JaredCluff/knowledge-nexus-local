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
#[allow(dead_code)] // consumed by P9+ background runner
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
#[allow(dead_code)] // consumed by P9+ background runner
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
#[allow(dead_code)] // consumed by P9+ background runner
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

use std::sync::Arc;
use anyhow::Result;

/// Result of a nightly tier transition pass.
#[allow(dead_code)] // consumed by P9+ background runner
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TransitionReport {
    pub articles_scanned: usize,
    pub articles_transitioned: usize,
    pub events_scanned: usize,
    pub events_transitioned: usize,
    /// Per-transition counts (e.g. "hot->warm": 5)
    pub transitions_by_type: std::collections::HashMap<String, usize>,
    pub pinned_skipped: usize,
}

/// Walk all articles + events for a store; compute salience; transition tier
/// when it changes AND the item is not pinned. Audit-logged via the Store
/// trait methods. Returns a per-pass report.
#[allow(dead_code)] // consumed by P9+ background runner
pub async fn nightly_tier_transition(
    db: Arc<dyn crate::store::Store>,
    store_id: &str,
    config: &DecayConfig,
    now: DateTime<Utc>,
) -> Result<TransitionReport> {
    let mut report = TransitionReport::default();

    // Articles
    let articles = db.list_articles_for_store(store_id).await?;
    report.articles_scanned = articles.len();

    for article in &articles {
        if article.pinned {
            report.pinned_skipped += 1;
            continue;
        }
        let s = salience(
            &SalienceInput {
                importance_score: article.importance_score,
                last_accessed_at: &article.last_accessed_at,
                created_at: &article.created_at,
                access_count: article.access_count,
                relevance: 0.0,
            },
            config,
            now,
        );
        let new_tier = tier_for_salience(s, config);
        if new_tier != article.tier {
            let from = tier_label(article.tier);
            let to = tier_label(new_tier);
            db.set_article_tier(
                &article.id,
                new_tier,
                &format!("nightly_decay: salience={:.4}", s),
            )
            .await?;
            *report
                .transitions_by_type
                .entry(format!("{}->{}", from, to))
                .or_insert(0) += 1;
            report.articles_transitioned += 1;
        }
    }

    // Events: parallel logic
    let events = db.list_events_for_store(store_id).await?;
    report.events_scanned = events.len();

    for event in &events {
        if event.pinned {
            report.pinned_skipped += 1;
            continue;
        }
        let s = salience(
            &SalienceInput {
                importance_score: event.importance_score,
                last_accessed_at: &event.last_accessed_at,
                created_at: &event.created_at,
                access_count: event.access_count,
                relevance: 0.0,
            },
            config,
            now,
        );
        let new_tier = tier_for_salience(s, config);
        if new_tier != event.tier {
            let from = tier_label(event.tier);
            let to = tier_label(new_tier);
            db.set_event_tier(
                &event.id,
                new_tier,
                &format!("nightly_decay: salience={:.4}", s),
            )
            .await?;
            *report
                .transitions_by_type
                .entry(format!("{}->{}", from, to))
                .or_insert(0) += 1;
            report.events_transitioned += 1;
        }
    }

    tracing::info!(
        "Nightly tier transition for store {}: articles {}/{} transitioned, events {}/{} transitioned, {} pinned skipped",
        store_id,
        report.articles_transitioned, report.articles_scanned,
        report.events_transitioned, report.events_scanned,
        report.pinned_skipped
    );

    Ok(report)
}

pub fn tier_label(t: crate::store::Tier) -> &'static str {
    match t {
        crate::store::Tier::Hot => "hot",
        crate::store::Tier::Warm => "warm",
        crate::store::Tier::Cold => "cold",
        crate::store::Tier::Archive => "archive",
    }
}

/// Retrieval-time tier weighting factor. Applied as a multiplicative
/// scalar on result confidence and PPR personalization weights.
///
/// Defaults are chosen so that:
/// - Hot items retain full weight
/// - Warm items are dampened to half (still surfaceable)
/// - Cold items are nearly invisible in default queries (0.1×)
/// - Archive items are excluded entirely unless include_archive=true.
pub fn tier_factor(tier: crate::store::Tier, include_archive: bool) -> f32 {
    use crate::store::Tier::*;
    match tier {
        Hot => 1.0,
        Warm => 0.5,
        Cold => 0.1,
        Archive => if include_archive { 0.05 } else { 0.0 },
    }
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
        let cfg = DecayConfig { formula: SalienceFormula::MemoryOsHeat, ..DecayConfig::default() };
        let now = epoch_2026();
        let low_visits = salience(&input_with("2026-05-24T00:00:00Z", 0.5, 1), &cfg, now);
        let high_visits = salience(&input_with("2026-05-24T00:00:00Z", 0.5, 100), &cfg, now);
        assert!(high_visits > low_visits,
            "more visits should yield higher heat; got {} vs {}", low_visits, high_visits);
    }

    #[test]
    fn memory_os_heat_clamped_to_unit_interval() {
        let cfg = DecayConfig { formula: SalienceFormula::MemoryOsHeat, ..DecayConfig::default() };
        let now = epoch_2026();
        let s = salience(&input_with("2026-05-24T00:00:00Z", 0.5, 1_000_000), &cfg, now);
        assert!((0.0..=1.0).contains(&s), "value out of range: {}", s);
    }

    #[test]
    fn generative_agents_weights_recency_relevance_importance() {
        let cfg = DecayConfig { formula: SalienceFormula::GenerativeAgents, ..DecayConfig::default() };
        let now = epoch_2026();
        // Fresh access + high importance + zero relevance: ga_w_recency·1 + 0 + ga_w_importance·1
        let s = salience(&input_with("2026-05-24T00:00:00Z", 1.0, 1), &cfg, now);
        let expected = cfg.ga_w_recency + cfg.ga_w_importance;  // recency=1, relevance=0, importance=1
        assert!((s - expected.min(1.0)).abs() < 1e-6,
            "expected {} (clamped), got {}", expected.min(1.0), s);
    }

    #[test]
    fn ebbinghaus_decays_with_time_and_strengthens_with_access() {
        let cfg = DecayConfig { formula: SalienceFormula::Ebbinghaus, ..DecayConfig::default() };
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
            let cfg = DecayConfig { formula, ..DecayConfig::default() };
            let s = salience(&input_with("2026-05-24T00:00:00Z", 0.5, 10), &cfg, now);
            assert!((0.0..=1.0).contains(&s),
                "{:?} produced out-of-range salience: {}", formula, s);
        }
    }

    use super::nightly_tier_transition;
    use crate::store::Store as _;

    fn build_aged_article(id: &str, days_ago: i64, importance: f64, pinned: bool) -> crate::store::Article {
        let now = chrono::Utc::now();
        let ts_old = (now - chrono::Duration::days(days_ago)).to_rfc3339();
        crate::store::Article {
            id: id.into(),
            store_id: "p8t5-s1".into(),
            title: format!("Article {}", id),
            content: String::new(),
            source_type: "user".into(),
            source_id: String::new(),
            content_hash: format!("{}-h", id),
            tags: serde_json::json!([]),
            embedded_at: None,
            created_at: ts_old.clone(),
            updated_at: ts_old.clone(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: ts_old,
            importance_score: importance,
            tier: Tier::Hot,
            pinned,
            compacted_into: None,
        }
    }

    #[tokio::test]
    async fn nightly_transition_demotes_old_articles() {
        let store = crate::store::SurrealStore::open_in_memory().await.unwrap();
        // Seed: fresh, mid-aged, old, ancient
        store.create_article(&build_aged_article("p8t5-fresh", 0, 0.8, false)).await.unwrap();
        store.create_article(&build_aged_article("p8t5-mid", 35, 0.8, false)).await.unwrap();
        store.create_article(&build_aged_article("p8t5-old", 150, 0.8, false)).await.unwrap();
        store.create_article(&build_aged_article("p8t5-ancient", 365, 0.8, false)).await.unwrap();

        let db: Arc<dyn crate::store::Store> = Arc::new(store);
        let cfg = DecayConfig::default();
        let now = chrono::Utc::now();

        let report = nightly_tier_transition(db.clone(), "p8t5-s1", &cfg, now).await.unwrap();

        assert_eq!(report.articles_scanned, 4);
        assert!(report.articles_transitioned >= 2,
            "expected mid/old/ancient to demote; got {} transitions", report.articles_transitioned);

        let fresh = db.get_article("p8t5-fresh").await.unwrap().unwrap();
        assert_eq!(fresh.tier, Tier::Hot, "fresh article should stay Hot");

        let ancient = db.get_article("p8t5-ancient").await.unwrap().unwrap();
        assert!(matches!(ancient.tier, Tier::Cold | Tier::Archive),
            "year-old article should be Cold or Archive; got {:?}", ancient.tier);
    }

    #[tokio::test]
    async fn nightly_transition_respects_pin() {
        let store = crate::store::SurrealStore::open_in_memory().await.unwrap();
        // Same fixture but the old article is pinned
        store.create_article(&build_aged_article("p8t5p-old", 365, 0.8, true)).await.unwrap();

        let db: Arc<dyn crate::store::Store> = Arc::new(store);
        let cfg = DecayConfig::default();
        let now = chrono::Utc::now();

        let report = nightly_tier_transition(db.clone(), "p8t5-s1", &cfg, now).await.unwrap();

        assert_eq!(report.pinned_skipped, 1);
        let pinned = db.get_article("p8t5p-old").await.unwrap().unwrap();
        assert_eq!(pinned.tier, Tier::Hot,
            "pinned article should never transition; got {:?}", pinned.tier);
    }

    #[tokio::test]
    async fn nightly_transition_no_op_on_empty_store() {
        let store = crate::store::SurrealStore::open_in_memory().await.unwrap();
        let db: Arc<dyn crate::store::Store> = Arc::new(store);
        let cfg = DecayConfig::default();
        let now = chrono::Utc::now();

        let report = nightly_tier_transition(db, "empty-store", &cfg, now).await.unwrap();
        assert_eq!(report.articles_scanned, 0);
        assert_eq!(report.events_scanned, 0);
        assert_eq!(report.articles_transitioned, 0);
    }

    #[tokio::test]
    async fn nightly_transition_writes_audit_log() {
        let store = crate::store::SurrealStore::open_in_memory().await.unwrap();
        store.create_article(&build_aged_article("p8t5a-ancient", 365, 0.8, false)).await.unwrap();

        let db: Arc<dyn crate::store::Store> = Arc::new(store);
        let cfg = DecayConfig::default();
        let now = chrono::Utc::now();

        let _ = nightly_tier_transition(db.clone(), "p8t5-s1", &cfg, now).await.unwrap();

        // Verify audit log has a tier_change entry
        let entries = db.list_audit_log("p8t5-s1", None, 100).await.unwrap();
        let has_transition = entries.iter().any(|e|
            e.action == "tier_change"
            && e.subject_id == "p8t5a-ancient"
            && e.details.get("reason")
                .and_then(|v| v.as_str())
                .map(|s| s.contains("nightly_decay"))
                .unwrap_or(false)
        );
        assert!(has_transition,
            "nightly_tier_transition should write audit entries; got entries: {:?}",
            entries);
    }
}
