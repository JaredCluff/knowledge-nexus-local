//! Task queue and executor for K2K multi-agent task delegation

use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use k2k::{
    TaskEvent, TaskEventType, TaskRequest, TaskResult, TaskStatus, TaskStatusResponse,
    TaskSubmitResponse,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, RwLock, Semaphore};
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::store::Store;

// ============================================================================
// Per-Client Rate Limiter
// ============================================================================

/// Sliding-window token tracker for a single client.
#[derive(Debug)]
struct ClientWindow {
    /// Timestamps of task submissions within the current window.
    timestamps: Vec<Instant>,
}

impl ClientWindow {
    fn new() -> Self {
        Self {
            timestamps: Vec::new(),
        }
    }

    /// Prune timestamps that have fallen outside the sliding window.
    fn prune(&mut self, window: Duration) {
        let now = Instant::now();
        self.timestamps.retain(|&t| now.duration_since(t) < window);
    }

    /// Check whether a new submission is allowed and, if so, record it.
    ///
    /// Returns `Ok(())` when allowed, `Err(retry_after_secs)` when the
    /// client has exhausted their window budget.
    fn check_and_record(&mut self, max_tasks: u32, window: Duration) -> Result<(), u64> {
        self.prune(window);
        if self.timestamps.len() as u32 >= max_tasks {
            // Estimate how many seconds until the oldest entry falls out.
            let oldest = self.timestamps[0];
            let age = Instant::now().duration_since(oldest);
            let retry = window
                .checked_sub(age)
                .unwrap_or(Duration::from_secs(1))
                .as_secs()
                + 1;
            return Err(retry);
        }
        self.timestamps.push(Instant::now());
        Ok(())
    }
}

/// Per-client rate limiter backed by a `HashMap<client_id, ClientWindow>`.
pub struct PerClientRateLimiter {
    windows: RwLock<HashMap<String, ClientWindow>>,
    max_tasks_per_window: u32,
    window_duration: Duration,
}

impl PerClientRateLimiter {
    pub fn new(max_tasks_per_window: u32, window_seconds: u64) -> Self {
        Self {
            windows: RwLock::new(HashMap::new()),
            max_tasks_per_window,
            window_duration: Duration::from_secs(window_seconds),
        }
    }

    /// Check the client's rate limit and record the attempt if allowed.
    ///
    /// Returns `Ok(())` when allowed, `Err(retry_after_secs)` when limited.
    pub async fn check(&self, client_id: &str) -> Result<(), u64> {
        let mut windows = self.windows.write().await;
        let window = windows
            .entry(client_id.to_string())
            .or_insert_with(ClientWindow::new);
        window.check_and_record(self.max_tasks_per_window, self.window_duration)
    }

    /// Remove stale windows to bound memory use.  Call periodically.
    pub async fn cleanup(&self) {
        let mut windows = self.windows.write().await;
        let dur = self.window_duration;
        windows.retain(|_, w| {
            w.prune(dur);
            !w.timestamps.is_empty()
        });
    }
}

// ============================================================================
// Task Handler Trait
// ============================================================================

/// Trait for implementing task handlers
#[async_trait]
pub trait TaskHandler: Send + Sync {
    async fn execute(
        &self,
        input: &serde_json::Value,
        progress_tx: mpsc::Sender<u8>,
    ) -> Result<serde_json::Value>;
}

// ============================================================================
// Internal State
// ============================================================================

/// Internal task state
#[allow(dead_code)]
struct TaskEntry {
    request: TaskRequest,
    status: TaskStatus,
    result: Option<TaskResult>,
    error: Option<String>,
    created_at: chrono::DateTime<Utc>,
    updated_at: chrono::DateTime<Utc>,
    progress: Option<u8>,
}

// ============================================================================
// TaskQueue
// ============================================================================

/// Configuration for per-client enforcement inside the task queue.
#[derive(Debug, Clone)]
pub struct TaskQueueLimits {
    /// Maximum tasks per window per client (0 = unlimited).
    pub max_tasks_per_window: u32,
    /// Sliding window duration in seconds.
    pub window_seconds: u64,
    /// Maximum articles a client may store (0 = unlimited).
    pub max_articles_per_client: usize,
}

impl Default for TaskQueueLimits {
    fn default() -> Self {
        Self {
            max_tasks_per_window: 10,
            window_seconds: 60,
            max_articles_per_client: 10_000,
        }
    }
}

pub struct TaskQueue {
    tasks: Arc<RwLock<HashMap<String, TaskEntry>>>,
    handlers: Arc<RwLock<HashMap<String, Arc<dyn TaskHandler>>>>,
    event_tx: broadcast::Sender<TaskEvent>,
    /// Semaphore that enforces max_concurrent — permits are acquired before
    /// execution begins and released automatically when the task finishes.
    semaphore: Arc<Semaphore>,
    /// Per-client rate limiter (10 tasks / 60 s by default).
    per_client_limiter: Arc<PerClientRateLimiter>,
    /// Limits config (for storage quota).
    limits: TaskQueueLimits,
    /// Database reference for article count queries.
    db: Option<Arc<dyn Store>>,
}

impl TaskQueue {
    pub fn new(max_concurrent: usize) -> Self {
        Self::with_limits(max_concurrent, TaskQueueLimits::default(), None)
    }

    pub fn with_limits(
        max_concurrent: usize,
        limits: TaskQueueLimits,
        db: Option<Arc<dyn Store>>,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        let per_client_limiter = Arc::new(PerClientRateLimiter::new(
            limits.max_tasks_per_window,
            limits.window_seconds,
        ));

        // Spawn periodic cleanup for stale client windows (every 5 minutes).
        let limiter_clone = Arc::clone(&per_client_limiter);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(300));
            loop {
                interval.tick().await;
                limiter_clone.cleanup().await;
            }
        });

        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            handlers: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
            semaphore: Arc::new(Semaphore::new(max_concurrent.max(1))),
            per_client_limiter,
            limits,
            db,
        }
    }

    /// Register a handler for a capability
    pub async fn register_handler(&self, capability_id: &str, handler: Arc<dyn TaskHandler>) {
        let mut handlers = self.handlers.write().await;
        handlers.insert(capability_id.to_string(), handler);
    }

    /// Submit a new task for execution
    pub async fn submit(&self, request: TaskRequest) -> Result<TaskSubmitResponse> {
        // Validate handler exists
        {
            let handlers = self.handlers.read().await;
            if !handlers.contains_key(&request.capability_id) {
                anyhow::bail!(
                    "No handler registered for capability: {}",
                    request.capability_id
                );
            }
        }

        // --- Per-client rate limit check ---
        let client_id = &request.client_id;
        if self.limits.max_tasks_per_window > 0 {
            if let Err(retry_after) = self.per_client_limiter.check(client_id).await {
                warn!(
                    "[TASKS] Client '{}' rate-limited: retry after {}s",
                    client_id, retry_after
                );
                // Propagate the retry-after value encoded in the error message so
                // the handler layer can set the Retry-After HTTP header.
                anyhow::bail!(
                    "RATE_LIMITED:{}:Client '{}' has exceeded the per-client task rate limit \
                     ({} tasks per {}s). Retry after {}s.",
                    retry_after,
                    client_id,
                    self.limits.max_tasks_per_window,
                    self.limits.window_seconds,
                    retry_after
                );
            }
        }

        // --- Per-client storage quota check (knowledge_store capability) ---
        if request.capability_id == "knowledge_store" && self.limits.max_articles_per_client > 0 {
            if let Some(ref db) = self.db {
                match db.count_articles_for_owner(client_id).await {
                    Ok(count) if count >= self.limits.max_articles_per_client => {
                        warn!(
                            "[TASKS] Client '{}' storage quota exceeded: {} >= {} articles",
                            client_id, count, self.limits.max_articles_per_client
                        );
                        anyhow::bail!(
                            "QUOTA_EXCEEDED:Client '{}' has reached the storage quota of {} articles \
                             ({} stored). Delete articles to free space.",
                            client_id,
                            self.limits.max_articles_per_client,
                            count
                        );
                    }
                    Ok(_) => {} // Under quota — proceed
                    Err(e) => {
                        // Log but don't block on DB errors (fail open for quota only).
                        warn!(
                            "[TASKS] Could not check article quota for client '{}': {}. Proceeding.",
                            client_id, e
                        );
                    }
                }
            }
        }

        let task_id = Uuid::new_v4().to_string();
        let now = Utc::now();

        let entry = TaskEntry {
            request: request.clone(),
            status: TaskStatus::Queued,
            result: None,
            error: None,
            created_at: now,
            updated_at: now,
            progress: None,
        };

        {
            let mut tasks = self.tasks.write().await;
            tasks.insert(task_id.clone(), entry);
        }

        // Emit queued event
        let _ = self.event_tx.send(TaskEvent {
            task_id: task_id.clone(),
            client_id: request.client_id.clone(),
            event_type: TaskEventType::StatusChanged,
            data: serde_json::json!({ "status": "queued" }),
            timestamp: now,
        });

        // Spawn task execution
        let task_id_clone = task_id.clone();
        let task_client_id = request.client_id.clone();
        let tasks = self.tasks.clone();
        let handlers = self.handlers.clone();
        let event_tx = self.event_tx.clone();
        let semaphore = Arc::clone(&self.semaphore);
        let timeout_seconds = request.timeout_seconds.unwrap_or(300);

        tokio::spawn(async move {
            // Acquire a concurrency slot; blocks until one is available.
            // The permit is held for the duration of the task and released
            // automatically when it drops — no manual counter needed.
            let Ok(_permit) = semaphore.acquire_owned().await else {
                // Semaphore was closed — queue is shutting down; discard task.
                return;
            };

            // Update status to running
            {
                let mut t = tasks.write().await;
                if let Some(entry) = t.get_mut(&task_id_clone) {
                    entry.status = TaskStatus::Running;
                    entry.updated_at = Utc::now();
                }
            }

            let _ = event_tx.send(TaskEvent {
                task_id: task_id_clone.clone(),
                client_id: task_client_id.clone(),
                event_type: TaskEventType::StatusChanged,
                data: serde_json::json!({ "status": "running" }),
                timestamp: Utc::now(),
            });

            // Execute
            let start = Instant::now();
            let (progress_tx, mut progress_rx) = mpsc::channel(32);

            let handler = {
                let h = handlers.read().await;
                h.get(&request.capability_id).cloned()
            };

            let handler = match handler {
                Some(h) => h,
                None => {
                    let mut t = tasks.write().await;
                    if let Some(entry) = t.get_mut(&task_id_clone) {
                        entry.status = TaskStatus::Failed;
                        entry.error = Some("Handler not found".to_string());
                        entry.updated_at = Utc::now();
                    }
                    // _permit drops here, releasing the concurrency slot.
                    return;
                }
            };

            // Spawn progress forwarder
            let progress_tasks = tasks.clone();
            let progress_event_tx = event_tx.clone();
            let progress_task_id = task_id_clone.clone();
            let progress_client_id = task_client_id.clone();
            tokio::spawn(async move {
                while let Some(pct) = progress_rx.recv().await {
                    {
                        let mut t = progress_tasks.write().await;
                        if let Some(entry) = t.get_mut(&progress_task_id) {
                            entry.progress = Some(pct);
                            entry.updated_at = Utc::now();
                        }
                    }
                    let _ = progress_event_tx.send(TaskEvent {
                        task_id: progress_task_id.clone(),
                        client_id: progress_client_id.clone(),
                        event_type: TaskEventType::Progress,
                        data: serde_json::json!({ "progress": pct }),
                        timestamp: Utc::now(),
                    });
                }
            });

            let result = tokio::time::timeout(
                std::time::Duration::from_secs(timeout_seconds),
                handler.execute(&request.input, progress_tx),
            )
            .await;

            let duration_ms = start.elapsed().as_millis() as u64;

            match result {
                Ok(Ok(data)) => {
                    let mut t = tasks.write().await;
                    if let Some(entry) = t.get_mut(&task_id_clone) {
                        entry.status = TaskStatus::Completed;
                        entry.result = Some(TaskResult {
                            data: data.clone(),
                            duration_ms,
                        });
                        entry.progress = Some(100);
                        entry.updated_at = Utc::now();
                    }
                    let _ = event_tx.send(TaskEvent {
                        task_id: task_id_clone.clone(),
                        client_id: task_client_id.clone(),
                        event_type: TaskEventType::Completed,
                        data: serde_json::json!({ "duration_ms": duration_ms }),
                        timestamp: Utc::now(),
                    });
                    info!(
                        "[TASKS] Task {} completed in {}ms",
                        task_id_clone, duration_ms
                    );
                }
                Ok(Err(e)) => {
                    let err_msg = e.to_string();
                    let mut t = tasks.write().await;
                    if let Some(entry) = t.get_mut(&task_id_clone) {
                        entry.status = TaskStatus::Failed;
                        entry.error = Some(err_msg.clone());
                        entry.updated_at = Utc::now();
                    }
                    let _ = event_tx.send(TaskEvent {
                        task_id: task_id_clone.clone(),
                        client_id: task_client_id.clone(),
                        event_type: TaskEventType::Failed,
                        data: serde_json::json!({ "error": err_msg }),
                        timestamp: Utc::now(),
                    });
                    error!("[TASKS] Task {} failed: {}", task_id_clone, err_msg);
                }
                Err(_) => {
                    let err_msg = format!("Task timed out after {}s", timeout_seconds);
                    let mut t = tasks.write().await;
                    if let Some(entry) = t.get_mut(&task_id_clone) {
                        entry.status = TaskStatus::Failed;
                        entry.error = Some(err_msg.clone());
                        entry.updated_at = Utc::now();
                    }
                    let _ = event_tx.send(TaskEvent {
                        task_id: task_id_clone.clone(),
                        client_id: task_client_id.clone(),
                        event_type: TaskEventType::Failed,
                        data: serde_json::json!({ "error": err_msg }),
                        timestamp: Utc::now(),
                    });
                    warn!("[TASKS] Task {} timed out", task_id_clone);
                }
            }
            // _permit drops here, automatically releasing the concurrency slot.
        });

        Ok(TaskSubmitResponse {
            task_id,
            status: TaskStatus::Queued,
        })
    }

    /// Get task status for a specific client.
    ///
    /// Returns `None` if the task does not exist **or** is owned by a different client.
    /// This prevents cross-client task status enumeration (IDOR).
    pub async fn get_status_for_client(
        &self,
        task_id: &str,
        requesting_client_id: &str,
    ) -> Option<TaskStatusResponse> {
        let tasks = self.tasks.read().await;
        let entry = tasks.get(task_id)?;
        if entry.request.client_id != requesting_client_id {
            return None;
        }
        Some(TaskStatusResponse {
            task_id: task_id.to_string(),
            status: entry.status.clone(),
            result: entry.result.clone(),
            error: entry.error.clone(),
            created_at: entry.created_at,
            updated_at: entry.updated_at,
            progress: entry.progress,
        })
    }

    /// Get task status (internal use only — no ownership check).
    pub async fn get_status(&self, task_id: &str) -> Option<TaskStatusResponse> {
        let tasks = self.tasks.read().await;
        tasks.get(task_id).map(|entry| TaskStatusResponse {
            task_id: task_id.to_string(),
            status: entry.status.clone(),
            result: entry.result.clone(),
            error: entry.error.clone(),
            created_at: entry.created_at,
            updated_at: entry.updated_at,
            progress: entry.progress,
        })
    }

    /// Cancel a task.
    ///
    /// Returns `None` if the task does not exist or is not owned by `requesting_client_id`.
    /// Returns `Some(true)` if cancelled; `Some(false)` if found but not in a cancellable state.
    pub async fn cancel_for_client(&self, task_id: &str, requesting_client_id: &str) -> Option<bool> {
        let mut tasks = self.tasks.write().await;
        let entry = tasks.get_mut(task_id)?;
        // Ownership check — treat another client's task as not found.
        if entry.request.client_id != requesting_client_id {
            return None;
        }
        if entry.status == TaskStatus::Queued || entry.status == TaskStatus::Running {
            let owner_client_id = entry.request.client_id.clone();
            entry.status = TaskStatus::Cancelled;
            entry.updated_at = Utc::now();
            let _ = self.event_tx.send(TaskEvent {
                task_id: task_id.to_string(),
                client_id: owner_client_id,
                event_type: TaskEventType::StatusChanged,
                data: serde_json::json!({ "status": "cancelled" }),
                timestamp: Utc::now(),
            });
            Some(true)
        } else {
            Some(false)
        }
    }

    /// Cancel a task (internal use only — no ownership check).
    pub async fn cancel(&self, task_id: &str) -> bool {
        let mut tasks = self.tasks.write().await;
        if let Some(entry) = tasks.get_mut(task_id) {
            if entry.status == TaskStatus::Queued || entry.status == TaskStatus::Running {
                let owner_client_id = entry.request.client_id.clone();
                entry.status = TaskStatus::Cancelled;
                entry.updated_at = Utc::now();
                let _ = self.event_tx.send(TaskEvent {
                    task_id: task_id.to_string(),
                    client_id: owner_client_id,
                    event_type: TaskEventType::StatusChanged,
                    data: serde_json::json!({ "status": "cancelled" }),
                    timestamp: Utc::now(),
                });
                return true;
            }
        }
        false
    }

    /// Subscribe to task events (for SSE endpoint)
    pub fn subscribe(&self) -> broadcast::Receiver<TaskEvent> {
        self.event_tx.subscribe()
    }
}
