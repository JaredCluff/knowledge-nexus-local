//! File search and indexing module.
//!
//! Provides semantic search over local files using embedded vector database
//! and ONNX embeddings.

mod indexer;
mod walker;

use anyhow::Result;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use crate::config::{Config, IndexingConfig};
use crate::embeddings::EmbeddingModel;
use crate::security::PathWhitelist;
use crate::vectordb::VectorDB;

/// Search result
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub id: String,
    pub path: String,
    pub filename: String,
    pub content: Option<String>,
    pub content_type: String,
    pub score: f32,
    pub snippet: Option<String>,
    pub metadata: Option<FileMetadata>,
}

/// File metadata
#[derive(Debug, Clone)]
pub struct FileMetadata {
    pub size_bytes: u64,
    pub modified_at: String,
    pub created_at: Option<String>,
    pub permissions: Option<String>,
}

/// Search files using semantic search
pub async fn search_files(config: &Config, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
    const MAX_QUERY_LENGTH: usize = 10_000;
    if query.len() > MAX_QUERY_LENGTH {
        anyhow::bail!("Query exceeds maximum length of {} characters", MAX_QUERY_LENGTH);
    }

    info!("Searching for: {} (limit: {})", query, limit);

    // Initialize components
    let mut embedding_model = EmbeddingModel::new()?;
    let vectordb = VectorDB::new().await?;

    // Generate query embedding
    let query_embedding = embedding_model.embed_text(query)?;

    // Search vector database
    let results = vectordb.search(&query_embedding, limit).await?;

    // Load whitelist to filter results
    let whitelist = PathWhitelist::new(
        config.security.allowed_paths.clone(),
        config.security.blocked_patterns.clone(),
        config.security.max_file_size,
    )?;

    // Filter and enhance results
    let mut search_results = Vec::new();
    for result in results {
        let path = PathBuf::from(&result.path);

        // Security check: ensure result is still in whitelist
        if !whitelist.is_allowed(&path) {
            warn!("Filtered out result not in whitelist: {}", result.path);
            continue;
        }

        // Load file content for snippet
        let snippet = if config.indexing.include_content {
            match std::fs::read_to_string(&path) {
                Ok(content) => extract_snippet(&content, query, 150),
                Err(_) => None,
            }
        } else {
            None
        };

        search_results.push(SearchResult {
            id: result.id,
            path: result.path,
            filename: path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            content: None,
            content_type: mime_guess::from_path(&path)
                .first_or_octet_stream()
                .to_string(),
            score: result.score,
            snippet,
            metadata: Some(FileMetadata {
                size_bytes: result.size_bytes,
                modified_at: result.modified_at,
                created_at: None,
                permissions: None,
            }),
        });
    }

    debug!("Found {} results", search_results.len());
    Ok(search_results)
}

/// Extract a snippet from content around matching terms
fn extract_snippet(content: &str, query: &str, max_len: usize) -> Option<String> {
    if content.is_empty() || max_len == 0 {
        return None;
    }

    let query_lower = query.to_lowercase();
    let content_lower = content.to_lowercase();

    // Find first occurrence of any query term
    let query_terms: Vec<&str> = query_lower.split_whitespace().collect();
    let mut best_pos = 0;

    for term in &query_terms {
        if let Some(pos) = content_lower.find(term) {
            best_pos = pos;
            break;
        }
    }

    // Extract snippet around match
    let start_guess = best_pos.saturating_sub(max_len / 2);
    let start = nearest_char_boundary(content, start_guess);
    let end = nearest_char_boundary(content, (start + max_len).min(content.len()));
    if start >= end {
        return None;
    }

    let snippet = &content[start..end];

    // Clean up snippet
    let snippet = snippet.trim();
    let snippet = snippet.replace('\n', " ").replace("  ", " ");

    if start > 0 {
        Some(format!("...{}", snippet))
    } else if end < content.len() {
        Some(format!("{}...", snippet))
    } else {
        Some(snippet.to_string())
    }
}

fn nearest_char_boundary(s: &str, mut idx: usize) -> usize {
    idx = idx.min(s.len());
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Start the file watcher for incremental indexing
pub async fn start_watcher(config: Config) -> Result<()> {
    indexer::start_watcher(config).await
}

/// Reindex all files
pub async fn reindex_all(config: &Config, force: bool) -> Result<()> {
    info!("Starting full reindex (force={})", force);

    let mut embedding_model = EmbeddingModel::new()?;
    let vectordb = VectorDB::new().await?;

    // Clear existing index if force
    if force {
        vectordb.clear().await?;
    }

    let whitelist = PathWhitelist::new(
        config.security.allowed_paths.clone(),
        config.security.blocked_patterns.clone(),
        config.security.max_file_size,
    )?;

    // Walk all allowed paths
    let mut indexed = 0;
    for allowed_path in whitelist.allowed_paths() {
        info!("Indexing: {}", allowed_path.display());

        for entry in walker::walk_directory(allowed_path, &config.indexing)? {
            let path = entry.path();

            // Skip if not allowed
            if !whitelist.is_allowed(path) {
                continue;
            }

            // Skip if excluded by indexing config
            if is_excluded_by_indexing(path, &config.indexing) {
                continue;
            }

            // Respect configured extension filter when set
            if !matches_extension_filter(path, &config.indexing.file_extensions) {
                continue;
            }

            // Skip if not text file
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            if mime.type_() != mime::TEXT && !is_code_file(path) {
                continue;
            }

            // Skip if too large
            let metadata = match std::fs::metadata(path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if metadata.len() > config.indexing.max_file_size {
                continue;
            }

            // Read content
            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // Generate embedding
            let embedding = match embedding_model.embed_text(&content) {
                Ok(e) => e,
                Err(e) => {
                    warn!("Failed to embed {}: {}", path.display(), e);
                    continue;
                }
            };

            let modified = metadata
                .modified()
                .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
                .unwrap_or_default();

            // Store in vector database
            vectordb
                .insert(
                    path.to_string_lossy().as_ref(),
                    &embedding,
                    metadata.len(),
                    &modified,
                )
                .await?;

            indexed += 1;
            if indexed % 100 == 0 {
                info!("Indexed {} files...", indexed);
            }
        }
    }

    info!("Indexing complete: {} files indexed", indexed);
    Ok(())
}

/// Check if file is a code file
fn is_code_file(path: &std::path::Path) -> bool {
    let code_extensions = [
        "rs", "py", "js", "ts", "jsx", "tsx", "go", "java", "c", "cpp", "h", "hpp", "rb", "php",
        "swift", "kt", "scala", "clj", "ex", "exs", "erl", "hs", "ml", "fs", "cs", "vb", "sql",
        "sh", "bash", "zsh", "fish", "ps1", "yaml", "yml", "json", "toml", "xml", "html", "css",
        "scss", "less", "md", "markdown", "rst", "txt", "cfg", "ini", "conf", "env",
    ];

    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| code_extensions.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

pub(super) fn matches_extension_filter(path: &Path, file_extensions: &[String]) -> bool {
    if file_extensions.is_empty() {
        return true;
    }

    let ext = match path.extension().and_then(|e| e.to_str()) {
        Some(e) => e.to_lowercase(),
        None => return false,
    };

    file_extensions.iter().any(|allowed| {
        let normalized = allowed.trim_start_matches('.').to_lowercase();
        normalized == ext
    })
}

pub(super) fn is_excluded_by_indexing(path: &Path, indexing: &IndexingConfig) -> bool {
    let path_str = path.to_string_lossy();

    for excluded in &indexing.exclude_paths {
        if crate::path_utils::str_path_contains(&path_str, excluded) {
            return true;
        }
    }

    for pattern in &indexing.exclude_patterns {
        if let Ok(glob) = glob::Pattern::new(pattern) {
            if glob.matches(&path_str) {
                return true;
            }
        }
    }

    false
}
