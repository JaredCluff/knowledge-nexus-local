//! File search and indexing module.
//!
//! Provides semantic search over local files and knowledge-store articles using
//! the tri-signal retrieval stack (vector + keyword + graph) via LocalRouter.

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

/// Search files and articles using tri-signal retrieval (vector + keyword + graph).
///
/// Delegates to `LocalRouter::route` and translates `K2KResult` into the
/// existing `SearchResult` shape so all callers (UI, chat handler, K2K query
/// handler) receive the same type without modification.
pub async fn search_files(config: &Config, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
    const MAX_QUERY_LENGTH: usize = 10_000;
    if query.len() > MAX_QUERY_LENGTH {
        anyhow::bail!("Query exceeds maximum length of {} characters", MAX_QUERY_LENGTH);
    }

    info!("Searching for: {} (limit: {})", query, limit);

    // Build the tri-signal retrieval stack (mirrors cmd_search from P4 Task 6).
    let db = crate::store::open_from_config(config).await?;
    let owner = db
        .get_owner_user()
        .await?
        .ok_or_else(|| anyhow::anyhow!("No owner user found. Run `init` first."))?;
    let stores = db.list_stores_for_user(&owner.id).await?;
    let default_store = stores
        .first()
        .ok_or_else(|| anyhow::anyhow!("No knowledge stores configured"))?;

    let registry = crate::vectordb::quantizer::QuantizerRegistry::new();
    let quantizer = registry.resolve(&default_store.quantizer_version)?;
    let vdb = std::sync::Arc::new(VectorDB::open(quantizer).await?);
    let emb = EmbeddingModel::new()?;
    let emb_arc = std::sync::Arc::new(tokio::sync::Mutex::new(emb));
    let hybrid = Some(std::sync::Arc::new(
        crate::retrieval::HybridSearcher::new(db.clone()),
    ));

    let router = crate::router::LocalRouter::new(
        db.clone(),
        vdb,
        emb_arc,
        hybrid,
        None,
        config.retrieval.clone(),
    );

    let response = router.route(query, &owner.id, None, limit).await?;

    // Optional path whitelist for "file"-source articles. Non-file articles
    // pass through (they have no path to validate).
    let whitelist = PathWhitelist::new(
        config.security.allowed_paths.clone(),
        config.security.blocked_patterns.clone(),
        config.security.max_file_size,
    )?;

    let mut out = Vec::with_capacity(response.results.len());
    for r in response.results {
        // Translate K2KResult → SearchResult. For "file" source_type articles,
        // the source_id is the file path; verify it's still in the whitelist and
        // fill file metadata where possible.
        let (path_str, filename, content_type, metadata) = if r.source_type == "file" {
            // Re-fetch the article to get source_id (the file path). This is an
            // extra round-trip but only for results that survive ranking.
            match db.get_article(&r.article_id).await? {
                Some(article) if !article.source_id.is_empty() => {
                    let p = PathBuf::from(&article.source_id);
                    if !whitelist.is_allowed(&p) {
                        warn!(
                            "Filtered out result not in whitelist: {}",
                            article.source_id
                        );
                        continue;
                    }
                    let fname = p
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let ct = mime_guess::from_path(&p)
                        .first_or_octet_stream()
                        .to_string();
                    let meta = std::fs::metadata(&p).ok().map(|m| FileMetadata {
                        size_bytes: m.len(),
                        modified_at: m
                            .modified()
                            .ok()
                            .map(|t| {
                                chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()
                            })
                            .unwrap_or_default(),
                        created_at: None,
                        permissions: None,
                    });
                    (article.source_id, fname, ct, meta)
                }
                _ => (String::new(), String::new(), "text/plain".to_string(), None),
            }
        } else {
            // Non-file articles: title doubles as filename, no file metadata.
            (
                r.article_id.clone(),
                r.title.clone(),
                "text/plain".to_string(),
                None,
            )
        };

        let snippet = if r.summary.is_empty() {
            None
        } else {
            Some(r.summary.clone())
        };

        out.push(SearchResult {
            id: r.article_id.clone(),
            path: path_str,
            filename,
            content: None,
            content_type,
            score: r.confidence,
            snippet,
            metadata,
        });
    }

    debug!("Found {} results", out.len());
    Ok(out)
}

/// Start the file watcher for incremental indexing
pub async fn start_watcher(config: Config) -> Result<()> {
    indexer::start_watcher(config).await
}

/// Reindex all files
pub async fn reindex_all(config: &Config, force: bool) -> Result<()> {
    info!("Starting full reindex (force={})", force);

    let mut embedding_model = EmbeddingModel::new()?;
    // TODO(P2): VectorDB::new() defaults to IvfPqQuantizer. Should resolve
    // from the store's quantizer_version once reindex_all() has access to store config.
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
