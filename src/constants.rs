//! Application-wide constants.
//!
//! Centralizes magic numbers and configuration defaults to make them
//! easier to find, modify, and understand.

// ============================================================================
// Network & Connection
// ============================================================================

/// Default WebSocket connection timeout in seconds
pub const WEBSOCKET_CONNECT_TIMEOUT_SECS: u64 = 30;

/// Default authentication timeout in seconds
pub const AUTH_TIMEOUT_SECS: u64 = 30;

/// Default heartbeat interval in seconds
pub const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// Initial reconnection delay in seconds
pub const RECONNECT_INITIAL_DELAY_SECS: f64 = 1.0;

/// Maximum reconnection delay in seconds
pub const RECONNECT_MAX_DELAY_SECS: f64 = 60.0;

// ============================================================================
// UI Server
// ============================================================================

/// Default port for the local UI web server
pub const DEFAULT_UI_PORT: u16 = 19850;

// ============================================================================
// Embeddings & ML
// ============================================================================

/// Embedding dimension for MiniLM-L6-v2 model
pub const EMBEDDING_DIM: usize = 384;

/// Maximum sequence length for the embedding model
pub const MAX_SEQUENCE_LENGTH: usize = 256;

/// HTTP timeout for model downloads in seconds
pub const MODEL_DOWNLOAD_TIMEOUT_SECS: u64 = 300;

/// HTTP connect timeout for model downloads in seconds
pub const MODEL_DOWNLOAD_CONNECT_TIMEOUT_SECS: u64 = 30;

// ============================================================================
// Security & Audit
// ============================================================================

/// Maximum number of days to retain audit logs
pub const AUDIT_LOG_RETENTION_DAYS: i64 = 30;

/// File permissions for audit logs (owner read/write only)
#[cfg(unix)]
pub const AUDIT_LOG_PERMISSIONS: u32 = 0o600;

// ============================================================================
// Indexing
// ============================================================================

/// Default maximum file size for indexing (10 MB)
pub const DEFAULT_MAX_FILE_SIZE: u64 = 10_485_760;

/// Debounce time for file watcher events in milliseconds
pub const FILE_WATCHER_DEBOUNCE_MS: u64 = 500;

/// Interval for logging indexing progress
pub const INDEX_PROGRESS_LOG_INTERVAL: usize = 100;

// ============================================================================
// Service Restart
// ============================================================================

/// Delay before restarting a failed service in seconds
pub const SERVICE_RESTART_DELAY_SECS: u64 = 5;
