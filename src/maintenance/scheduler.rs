//! Maintenance job scheduler with idempotency-key deduplication and
//! restart-safe interval tracking.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::store::Store;

/// Idempotency-key generator: produces a stable key for a (job, time-window) pair.
/// Returning the same key for two invocations means they're considered duplicate.
pub type IdempotencyKeyFn = Arc<dyn Fn() -> String + Send + Sync>;

/// Async job handler. Returns Ok(()) on success; Err for permanent failure.
pub type JobHandler = Arc<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send>>
        + Send
        + Sync,
>;

/// Specification for a registered job.
pub struct JobSpec {
    pub name: String,
    /// Minimum interval between runs (idempotency window).
    pub min_interval: Duration,
    /// Generates the idempotency key for the current invocation.
    pub idempotency_key: IdempotencyKeyFn,
    /// The job body.
    pub handler: JobHandler,
}

/// Scheduler holds registered jobs and provides on-demand or interval-driven
/// execution. NOT a separate background task (yet) — for P7 this is a
/// foreground-only API used by the CLI and ingest paths. P8 may add a
/// long-lived tokio task; for P7 we keep it minimal.
pub struct MaintenanceScheduler {
    db: Arc<dyn Store>,
    jobs: HashMap<String, JobSpec>,
}

impl MaintenanceScheduler {
    pub fn new(db: Arc<dyn Store>) -> Self {
        Self {
            db,
            jobs: HashMap::new(),
        }
    }

    /// Register a job. Replaces any existing registration with the same name.
    pub fn register(&mut self, spec: JobSpec) {
        self.jobs.insert(spec.name.clone(), spec);
    }

    /// Run a job by name, respecting idempotency. Returns:
    /// - `Ok(true)` if the job ran (handler invoked)
    /// - `Ok(false)` if skipped (duplicate idempotency key within min_interval)
    /// - `Err(_)` if the handler failed or the DB write failed
    pub async fn run_once(&self, job_name: &str) -> Result<bool> {
        let spec = self
            .jobs
            .get(job_name)
            .ok_or_else(|| anyhow::anyhow!("Job not registered: {}", job_name))?;

        let key = (spec.idempotency_key)();
        let now = chrono::Utc::now().to_rfc3339();

        // Check idempotency: was this exact key used recently?
        if self.was_run_recently(&key, spec.min_interval).await? {
            tracing::info!(
                "Maintenance: skipping {} (idempotency key {:?} within min_interval)",
                job_name,
                key
            );
            return Ok(false);
        }

        // Record the run as in-progress
        self.record_run_start(job_name, &key, &now).await?;

        // Execute the job
        let result = (spec.handler)().await;

        let completed_at = chrono::Utc::now().to_rfc3339();
        let status = if result.is_ok() { "completed" } else { "failed" };
        self.record_run_end(&key, &completed_at, status).await?;

        result?;
        Ok(true)
    }

    /// Check if an idempotency key was used within the last `interval`.
    async fn was_run_recently(&self, key: &str, interval: Duration) -> Result<bool> {
        let cutoff = chrono::Utc::now()
            - chrono::Duration::from_std(interval)
                .context("interval too large to convert to chrono::Duration")?;
        let cutoff_str = cutoff.to_rfc3339();

        self.db
            .recent_maintenance_run_by_key(key, &cutoff_str)
            .await
    }

    async fn record_run_start(&self, job_name: &str, key: &str, started_at: &str) -> Result<()> {
        self.db
            .record_maintenance_run(job_name, key, started_at)
            .await
    }

    async fn record_run_end(&self, key: &str, completed_at: &str, status: &str) -> Result<()> {
        self.db
            .complete_maintenance_run(key, completed_at, status)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SurrealStore;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn run_once_invokes_handler_first_time() {
        let store = SurrealStore::open_in_memory().await.unwrap();
        let db: Arc<dyn Store> = Arc::new(store);
        let mut scheduler = MaintenanceScheduler::new(db);

        let counter = Arc::new(AtomicUsize::new(0));
        let cnt_for_handler = Arc::clone(&counter);

        scheduler.register(JobSpec {
            name: "test_job".into(),
            min_interval: Duration::from_secs(60),
            idempotency_key: Arc::new(|| "test_key_1".into()),
            handler: Arc::new(move || {
                let c = Arc::clone(&cnt_for_handler);
                Box::pin(async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }),
        });

        let ran = scheduler.run_once("test_job").await.unwrap();
        assert!(ran, "first invocation should run");
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn run_once_skips_within_min_interval() {
        let store = SurrealStore::open_in_memory().await.unwrap();
        let db: Arc<dyn Store> = Arc::new(store);
        let mut scheduler = MaintenanceScheduler::new(db);

        let counter = Arc::new(AtomicUsize::new(0));
        let cnt_for_handler = Arc::clone(&counter);

        scheduler.register(JobSpec {
            name: "dedup_job".into(),
            min_interval: Duration::from_secs(60),
            idempotency_key: Arc::new(|| "dedup_key_2".into()),
            handler: Arc::new(move || {
                let c = Arc::clone(&cnt_for_handler);
                Box::pin(async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }),
        });

        let ran1 = scheduler.run_once("dedup_job").await.unwrap();
        let ran2 = scheduler.run_once("dedup_job").await.unwrap();
        assert!(ran1, "first run executes");
        assert!(!ran2, "second run within min_interval should skip");
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn unregistered_job_errors() {
        let store = SurrealStore::open_in_memory().await.unwrap();
        let db: Arc<dyn Store> = Arc::new(store);
        let scheduler = MaintenanceScheduler::new(db);
        let result = scheduler.run_once("nonexistent").await;
        assert!(result.is_err());
    }
}
