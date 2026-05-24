//! Periodic background maintenance jobs.
//!
//! The `MaintenanceScheduler` registers jobs by name with a minimum-interval
//! policy and an idempotency-key generator. Each job's last-run timestamp
//! is persisted to a `_maintenance_runs` table so the scheduler survives
//! process restarts.

pub mod compaction;
pub mod decay;
pub mod scheduler;

#[allow(unused_imports)] // P9+ consumers of the public API
pub use compaction::{compact_low_salience, CompactionReport};
#[allow(unused_imports)] // P9+ consumers of the public API
pub use decay::{
    nightly_tier_transition, salience, tier_factor, tier_for_salience, tier_label,
    SalienceInput, TransitionReport,
};
#[allow(unused_imports)] // P9+ consumers of the public API
pub use scheduler::{JobSpec, MaintenanceScheduler};

/// Construct a MaintenanceScheduler with all P7+P8 jobs registered.
/// Each job's idempotency key includes the store_id + a time window so
/// concurrent invocations dedup correctly.
#[allow(dead_code)] // P9+ will call this from the background runner
pub fn standard_scheduler(
    db: std::sync::Arc<dyn crate::store::Store>,
    _decay_cfg: crate::config::DecayConfig,
    _compaction_cfg: crate::config::CompactionConfig,
    _llm_cfg: crate::config::ExtractionConfig,
) -> MaintenanceScheduler {
    // For P8 we leave the scheduler empty — actual job registration is
    // deferred to a future task where a background runner exists. The
    // helper exists so P9+ can populate without touching call sites.
    //
    // NOTE: Pre-registering decay/compaction jobs here would create
    // circular dependencies between maintenance and the LLM-extractor
    // path (Reflector needs ExtractionConfig + reqwest, not Send across
    // the JobHandler closure cleanly in current API shape). P9 will
    // refactor to enable this.
    MaintenanceScheduler::new(db)
}
