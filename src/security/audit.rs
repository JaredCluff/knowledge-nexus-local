//! Audit Logger - Local logging of all file access operations.
//!
//! All file accesses are logged locally for security auditing.
//! Logs are append-only and protected with restricted permissions.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use uuid::Uuid;

use crate::config;

/// Audit log entry
#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct AuditEntry {
    pub timestamp: DateTime<Utc>,
    pub event_id: String,
    pub event_type: String,
    pub session_id: Option<String>,
    pub user_id: Option<String>,
    pub device_id: String,
    pub operation: String,
    pub target_path: Option<String>,
    pub query: Option<String>,
    pub status: String,
    pub denial_reason: Option<String>,
    pub execution_time_ms: u64,
    pub result_count: Option<usize>,
}

/// Audit logger
#[allow(dead_code)]
pub struct AuditLogger {
    log_file: Mutex<File>,
    device_id: String,
}

#[allow(dead_code)]
impl AuditLogger {
    /// Create a new audit logger
    pub fn new(device_id: String) -> Result<Self> {
        let log_path = Self::log_path();

        // Ensure parent directory exists
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;

        // Set restrictive permissions
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&log_path, perms)?;
        }
        #[cfg(windows)]
        {
            let path_str = log_path.to_string_lossy();
            let username = std::env::var("USERNAME").unwrap_or_else(|_| "".to_string());
            if !username.is_empty() {
                // Remove inherited permissions and grant only current user
                match std::process::Command::new("icacls")
                    .args([&*path_str, "/inheritance:r", "/grant", &format!("{}:RW", username)])
                    .output()
                {
                    Ok(output) if !output.status.success() => {
                        tracing::warn!(
                            "icacls failed for {}: exit code {:?}, stderr: {}",
                            path_str,
                            output.status.code(),
                            String::from_utf8_lossy(&output.stderr)
                        );
                    }
                    Err(e) => {
                        tracing::warn!("Failed to run icacls for {}: {}", path_str, e);
                    }
                    _ => {}
                }
            }
        }

        Ok(Self {
            log_file: Mutex::new(file),
            device_id,
        })
    }

    /// Get audit log path
    fn log_path() -> PathBuf {
        let today = Utc::now().format("%Y-%m-%d");
        config::data_dir()
            .join("audit")
            .join(format!("audit-{}.jsonl", today))
    }

    /// Log an entry
    pub fn log(&self, entry: AuditEntry) -> Result<()> {
        let json = serde_json::to_string(&entry)?;

        // Recover from poisoned mutex - audit logging should be resilient
        // and not fail permanently if another thread panicked
        let mut file = match self.log_file.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!("Audit log mutex was poisoned, recovering");
                poisoned.into_inner()
            }
        };
        writeln!(file, "{}", json)?;
        file.flush()?;

        Ok(())
    }

    /// Log a query event
    #[allow(clippy::too_many_arguments)]
    pub fn log_query(
        &self,
        session_id: Option<&str>,
        user_id: Option<&str>,
        query: &str,
        paths: &[String],
        status: &str,
        result_count: usize,
        denial_reason: Option<&str>,
        execution_time_ms: u64,
    ) {
        let entry = AuditEntry {
            timestamp: Utc::now(),
            event_id: Uuid::new_v4().to_string(),
            event_type: "query".to_string(),
            session_id: session_id.map(|s| s.to_string()),
            user_id: user_id.map(|s| s.to_string()),
            device_id: self.device_id.clone(),
            operation: "search".to_string(),
            target_path: Some(paths.join(",")),
            query: Some(query.to_string()),
            status: status.to_string(),
            denial_reason: denial_reason.map(|s| s.to_string()),
            execution_time_ms,
            result_count: Some(result_count),
        };

        if let Err(e) = self.log(entry) {
            tracing::error!("Failed to write audit log: {}", e);
        }
    }

    /// Log a file access event
    pub fn log_file_access(
        &self,
        session_id: Option<&str>,
        user_id: Option<&str>,
        path: &str,
        operation: &str,
        status: &str,
        denial_reason: Option<&str>,
    ) {
        let entry = AuditEntry {
            timestamp: Utc::now(),
            event_id: Uuid::new_v4().to_string(),
            event_type: "file_access".to_string(),
            session_id: session_id.map(|s| s.to_string()),
            user_id: user_id.map(|s| s.to_string()),
            device_id: self.device_id.clone(),
            operation: operation.to_string(),
            target_path: Some(path.to_string()),
            query: None,
            status: status.to_string(),
            denial_reason: denial_reason.map(|s| s.to_string()),
            execution_time_ms: 0,
            result_count: None,
        };

        if let Err(e) = self.log(entry) {
            tracing::error!("Failed to write audit log: {}", e);
        }
    }

    /// Log an authentication event
    pub fn log_auth(&self, status: &str, user_id: Option<&str>, error: Option<&str>) {
        let entry = AuditEntry {
            timestamp: Utc::now(),
            event_id: Uuid::new_v4().to_string(),
            event_type: "auth".to_string(),
            session_id: None,
            user_id: user_id.map(|s| s.to_string()),
            device_id: self.device_id.clone(),
            operation: "authenticate".to_string(),
            target_path: None,
            query: None,
            status: status.to_string(),
            denial_reason: error.map(|s| s.to_string()),
            execution_time_ms: 0,
            result_count: None,
        };

        if let Err(e) = self.log(entry) {
            tracing::error!("Failed to write audit log: {}", e);
        }
    }
}
