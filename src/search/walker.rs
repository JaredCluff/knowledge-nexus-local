//! Directory walker for file indexing.

use anyhow::Result;
use std::path::Path;
use walkdir::{DirEntry, WalkDir};

use crate::config::IndexingConfig;

/// Walk a directory recursively
pub fn walk_directory<'a>(
    root: &Path,
    config: &'a IndexingConfig,
) -> Result<impl Iterator<Item = DirEntry> + 'a> {
    let walker = WalkDir::new(root)
        .follow_links(config.follow_symlinks)
        .max_depth(config.max_depth.unwrap_or(usize::MAX))
        .into_iter()
        .filter_entry(|e| !is_hidden(e) && !is_ignored(e, config))
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file());

    Ok(walker)
}

/// Check if entry is hidden
fn is_hidden(entry: &DirEntry) -> bool {
    // Unix convention: files starting with '.' are hidden
    let dot_hidden = entry
        .file_name()
        .to_str()
        .map(|s| s.starts_with('.'))
        .unwrap_or(false);

    if dot_hidden {
        return true;
    }

    // Windows: check the FILE_ATTRIBUTE_HIDDEN flag
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
        if let Ok(meta) = entry.metadata() {
            let attrs = meta.file_attributes();
            if attrs & FILE_ATTRIBUTE_HIDDEN != 0 || attrs & FILE_ATTRIBUTE_SYSTEM != 0 {
                return true;
            }
        }
    }

    false
}

/// Check if entry should be ignored based on config
fn is_ignored(entry: &DirEntry, config: &IndexingConfig) -> bool {
    let path = entry.path();

    // Resolve symlinks to canonical paths before comparing against exclude list.
    // This prevents symlink escapes where a symlink inside an allowed directory
    // points to an excluded location (the string-based check would miss it).
    let canonical_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let path_str = canonical_path.to_string_lossy();

    // Check excluded paths using canonical (resolved) paths
    for excluded in &config.exclude_paths {
        let canonical_excluded = std::fs::canonicalize(excluded)
            .unwrap_or_else(|_| std::path::PathBuf::from(excluded));
        let excluded_str = canonical_excluded.to_string_lossy();
        if crate::path_utils::str_path_starts_with(path_str.as_ref(), excluded_str.as_ref()) {
            return true;
        }
    }

    // Check excluded patterns
    for pattern in &config.exclude_patterns {
        if let Ok(glob) = glob::Pattern::new(pattern) {
            if glob.matches(&path_str) {
                return true;
            }
        }
    }

    // Default ignored directories
    const IGNORED_DIRS: &[&str] = &[
        "node_modules",
        "target",
        "build",
        "dist",
        "out",
        ".git",
        ".svn",
        ".hg",
        ".bzr",
        "__pycache__",
        ".pytest_cache",
        ".mypy_cache",
        "venv",
        ".venv",
        "env",
        ".env",
        ".cargo",
        ".rustup",
        "vendor",
        "third_party",
    ];

    if entry.file_type().is_dir() {
        let name = entry.file_name().to_string_lossy();
        if IGNORED_DIRS.contains(&name.as_ref()) {
            return true;
        }
    }

    false
}
