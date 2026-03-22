//! Cross-platform path comparison utilities.
//!
//! On Windows, filesystem paths are case-insensitive, so path comparisons
//! must normalize case. On Unix, paths are case-sensitive and compared as-is.

use std::path::Path;

/// Check if `path` starts with `base`, using case-insensitive comparison on Windows.
pub fn path_starts_with(path: &Path, base: &Path) -> bool {
    #[cfg(windows)]
    {
        let path_lower = path.to_string_lossy().to_lowercase();
        let base_lower = base.to_string_lossy().to_lowercase();
        path_lower.starts_with(&base_lower)
    }
    #[cfg(not(windows))]
    {
        path.starts_with(base)
    }
}

/// Check if `haystack` starts with `needle` as strings, case-insensitive on Windows.
pub fn str_path_starts_with(haystack: &str, needle: &str) -> bool {
    #[cfg(windows)]
    {
        haystack.to_lowercase().starts_with(&needle.to_lowercase())
    }
    #[cfg(not(windows))]
    {
        haystack.starts_with(needle)
    }
}

/// Check if `haystack` contains `needle` as a substring, case-insensitive on Windows.
pub fn str_path_contains(haystack: &str, needle: &str) -> bool {
    #[cfg(windows)]
    {
        haystack.to_lowercase().contains(&needle.to_lowercase())
    }
    #[cfg(not(windows))]
    {
        haystack.contains(needle)
    }
}
