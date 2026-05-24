//! Periodic background maintenance jobs.
//!
//! The `MaintenanceScheduler` registers jobs by name with a minimum-interval
//! policy and an idempotency-key generator. Each job's last-run timestamp
//! is persisted to a `_maintenance_runs` table so the scheduler survives
//! process restarts.

pub mod decay;
pub mod scheduler;

pub use decay::{
    nightly_tier_transition, salience, tier_for_salience,
    SalienceInput, TransitionReport,
};
pub use scheduler::{JobSpec, MaintenanceScheduler};
