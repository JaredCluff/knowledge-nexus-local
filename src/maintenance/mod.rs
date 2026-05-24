//! Periodic background maintenance jobs.
//!
//! The `MaintenanceScheduler` registers jobs by name with a minimum-interval
//! policy and an idempotency-key generator. Each job's last-run timestamp
//! is persisted to a `_maintenance_runs` table so the scheduler survives
//! process restarts.

pub mod compaction;
pub mod decay;
pub mod scheduler;

pub use compaction::{compact_low_salience, CompactionReport};
pub use decay::{
    nightly_tier_transition, salience, tier_factor, tier_for_salience, tier_label,
    SalienceInput, TransitionReport,
};
pub use scheduler::{JobSpec, MaintenanceScheduler};
