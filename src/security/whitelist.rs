//! Path Whitelist - Immutable local security controls.
//!
//! SECURITY CRITICAL: This is the last line of defense against
//! unauthorized file access. Configuration is loaded locally and
//! cannot be modified by the server.

use anyhow::Result;
use glob::Pattern;
use std::path::{Path, PathBuf};
use tracing::warn;

/// Dangerous patterns that indicate path traversal attacks
const TRAVERSAL_PATTERNS: &[&str] = &["..", "%2e%2e", "%252e%252e"];

/// Path whitelist enforcer
pub struct PathWhitelist {
    allowed_paths: Vec<PathBuf>,
    blocked_patterns: Vec<Pattern>,
    #[allow(dead_code)]
    max_file_size: u64,
}

impl PathWhitelist {
    /// Create a new whitelist from configuration
    pub fn new(
        allowed_paths: Vec<String>,
        blocked_patterns: Vec<String>,
        max_file_size: u64,
    ) -> Result<Self> {
        let allowed_paths: Vec<PathBuf> = allowed_paths
            .into_iter()
            .filter_map(|p| {
                let _path = PathBuf::from(&p);
                let expanded = shellexpand::tilde(&p).to_string();
                let expanded_path = PathBuf::from(expanded);

                if expanded_path.exists() {
                    Some(expanded_path.canonicalize().unwrap_or(expanded_path))
                } else {
                    warn!("Allowed path does not exist: {}", p);
                    None
                }
            })
            .collect();

        let blocked_patterns: Vec<Pattern> = blocked_patterns
            .into_iter()
            .filter_map(|p| Pattern::new(&p).ok())
            .collect();

        Ok(Self {
            allowed_paths,
            blocked_patterns,
            max_file_size,
        })
    }

    /// Check if a path is allowed for access
    pub fn is_allowed(&self, path: &Path) -> bool {
        // Reject null bytes before any canonicalization attempt
        if path.to_string_lossy().contains('\0') {
            warn!("Null byte in path, rejecting");
            return false;
        }

        // Normalize and resolve the path
        let canonical = match path.canonicalize() {
            Ok(p) => p,
            Err(_) => {
                // If path doesn't exist, try to check the parent
                if let Some(parent) = path.parent() {
                    if let Ok(p) = parent.canonicalize() {
                        p.join(path.file_name().unwrap_or_default())
                    } else {
                        return false;
                    }
                } else {
                    return false;
                }
            }
        };

        let path_str = canonical.to_string_lossy();

        // Check for path traversal patterns
        if self.has_traversal_pattern(&path_str) {
            warn!("Path traversal detected: {}", path_str);
            return false;
        }

        // Check blocked patterns FIRST (highest priority)
        if self.matches_blocked_pattern(&canonical) {
            return false;
        }

        // Check if path is within allowed directories
        self.is_in_allowed_directory(&canonical)
    }

    /// Check for path traversal attacks
    fn has_traversal_pattern(&self, path: &str) -> bool {
        let path_lower = path.to_lowercase();
        TRAVERSAL_PATTERNS.iter().any(|p| path_lower.contains(p))
    }

    /// Check if path matches any blocked pattern
    fn matches_blocked_pattern(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();
        let file_name = path
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default();

        // On Windows, normalize to lowercase for case-insensitive matching
        #[cfg(windows)]
        let (path_str, file_name) = (
            std::borrow::Cow::Owned(path_str.to_lowercase()),
            std::borrow::Cow::Owned(file_name.to_lowercase()),
        );

        for pattern in &self.blocked_patterns {
            // Check full path
            if pattern.matches(&path_str) {
                return true;
            }
            // Check filename only
            if pattern.matches(&file_name) {
                return true;
            }
        }

        false
    }

    /// Check if path is within an allowed directory
    fn is_in_allowed_directory(&self, path: &Path) -> bool {
        for allowed in &self.allowed_paths {
            #[cfg(not(windows))]
            {
                if path.starts_with(allowed) {
                    return true;
                }
            }
            #[cfg(windows)]
            {
                // Case-insensitive comparison on Windows
                let path_lower = path.to_string_lossy().to_lowercase();
                let allowed_lower = allowed.to_string_lossy().to_lowercase();
                if path_lower.starts_with(&allowed_lower) {
                    return true;
                }
            }
        }
        false
    }

    /// Check if file size is within limits
    #[allow(dead_code)]
    pub fn check_file_size(&self, path: &Path) -> bool {
        match std::fs::metadata(path) {
            Ok(meta) => meta.len() <= self.max_file_size,
            Err(_) => false,
        }
    }

    /// Get list of allowed paths
    pub fn allowed_paths(&self) -> &[PathBuf] {
        &self.allowed_paths
    }

    /// Sanitize a user-provided path
    #[allow(dead_code)]
    pub fn sanitize_path(&self, user_path: &str, base_path: &Path) -> Option<PathBuf> {
        // Reject null bytes
        if user_path.contains('\0') {
            warn!("Null byte in path");
            return None;
        }

        // URL decode first (before normalizing separators, so encoded
        // backslashes like %5C are decoded before normalization)
        let decoded = match urlencoding::decode(user_path) {
            Ok(d) => d.to_string(),
            Err(_) => user_path.to_string(),
        };

        // Normalize path separators after decoding
        let decoded = decoded.replace('\\', "/");

        // Check for double encoding
        if let Ok(double_decoded) = urlencoding::decode(&decoded) {
            if double_decoded != decoded {
                warn!("Double encoding detected");
                return None;
            }
        }

        // Resolve against base path
        let target = base_path.join(&decoded);

        // Verify still within base (prevents ../ attacks)
        let canonical = match target.canonicalize() {
            Ok(p) => p,
            Err(_) => return None,
        };

        if !canonical.starts_with(base_path) {
            warn!("Path escapes base directory: {}", user_path);
            return None;
        }

        // Final whitelist check
        if !self.is_allowed(&canonical) {
            return None;
        }

        Some(canonical)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use tempfile::tempdir;

    #[test]
    fn test_allowed_path() -> Result<()> {
        let dir = tempdir()?;
        let allowed = dir.path().to_path_buf();

        let whitelist = PathWhitelist::new(
            vec![allowed.to_string_lossy().to_string()],
            vec!["*.env".to_string()],
            1024 * 1024,
        )?;

        // Create test file
        let test_file = allowed.join("test.txt");
        std::fs::write(&test_file, "test")?;

        assert!(whitelist.is_allowed(&test_file));
        Ok(())
    }

    #[test]
    fn test_blocked_pattern() -> Result<()> {
        let dir = tempdir()?;
        let allowed = dir.path().to_path_buf();

        let whitelist = PathWhitelist::new(
            vec![allowed.to_string_lossy().to_string()],
            vec!["*.env".to_string()],
            1024 * 1024,
        )?;

        // Create .env file
        let env_file = allowed.join(".env");
        std::fs::write(&env_file, "SECRET=test")?;

        assert!(!whitelist.is_allowed(&env_file));
        Ok(())
    }

    #[test]
    fn test_traversal_blocked() -> Result<()> {
        let dir = tempdir()?;
        let allowed = dir.path().to_path_buf();

        let whitelist = PathWhitelist::new(
            vec![allowed.to_string_lossy().to_string()],
            vec![],
            1024 * 1024,
        )?;

        let traversal_path = allowed.join("../../../etc/passwd");
        assert!(!whitelist.is_allowed(&traversal_path));
        Ok(())
    }
}
