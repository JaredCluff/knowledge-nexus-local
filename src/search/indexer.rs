//! File watcher and incremental indexer.

use anyhow::Result;
use notify::{Config as NotifyConfig, Event, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

use super::{is_code_file, is_excluded_by_indexing, matches_extension_filter};
use crate::config::Config;
use crate::embeddings::EmbeddingModel;
use crate::security::PathWhitelist;
use crate::vectordb::VectorDB;

/// Start the file watcher for incremental indexing
pub async fn start_watcher(config: Config) -> Result<()> {
    info!("Starting file watcher...");

    let whitelist = PathWhitelist::new(
        config.security.allowed_paths.clone(),
        config.security.blocked_patterns.clone(),
        config.security.max_file_size,
    )?;

    // Bounded channel for file events — backpressure prevents memory growth
    // if events arrive faster than we can process them.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Event>(1000);

    // Create watcher
    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| match res {
            Ok(event) => {
                if tx.try_send(event).is_err() {
                    error!("Failed to send file event (channel full or closed)");
                }
            }
            Err(e) => {
                error!("Watch error: {}", e);
            }
        },
        NotifyConfig::default()
            .with_poll_interval(Duration::from_millis(config.indexing.debounce_ms as u64)),
    )?;

    // Watch all allowed paths
    for path in whitelist.allowed_paths() {
        info!("Watching: {}", path.display());
        watcher.watch(path, RecursiveMode::Recursive)?;
    }

    // Initialize components
    let mut embedding_model = EmbeddingModel::new()?;
    // TODO(P2): VectorDB::new() defaults to IvfPqQuantizer. Should resolve
    // from the store's quantizer_version once start_watcher() has access to store config.
    let vectordb = VectorDB::new().await?;

    // Process events
    let mut last_processed: HashMap<PathBuf, Instant> = HashMap::new();
    let debounce = Duration::from_millis(config.indexing.debounce_ms as u64);
    loop {
        match rx.recv().await {
            Some(event) => {
                for path in event.paths {
                    let now = Instant::now();
                    if let Some(prev) = last_processed.get(&path) {
                        if now.duration_since(*prev) < debounce {
                            debug!("Debounced event for {}", path.display());
                            continue;
                        }
                    }
                    last_processed.insert(path.clone(), now);

                    process_file_event(
                        &path,
                        &event.kind,
                        &config,
                        &whitelist,
                        &mut embedding_model,
                        &vectordb,
                    )
                    .await;
                }
            }
            None => break,
        }
    }

    Ok(())
}

/// Process a file event
async fn process_file_event(
    path: &PathBuf,
    kind: &notify::EventKind,
    config: &Config,
    whitelist: &PathWhitelist,
    embedding_model: &mut EmbeddingModel,
    vectordb: &VectorDB,
) {
    use notify::EventKind::*;

    // Skip if not in whitelist
    if !whitelist.is_allowed(path) {
        return;
    }

    // Skip if excluded by indexing config
    if is_excluded_by_indexing(path, &config.indexing) {
        return;
    }

    // Respect configured extension filter when set
    if !matches_extension_filter(path, &config.indexing.file_extensions) {
        return;
    }

    match kind {
        Create(_) | Modify(_) => {
            debug!("File changed: {}", path.display());
            if let Err(e) = index_file(path, config, embedding_model, vectordb).await {
                warn!("Failed to index {}: {}", path.display(), e);
            }
        }
        Remove(_) => {
            debug!("File removed: {}", path.display());
            if let Err(e) = vectordb.delete(path.to_string_lossy().as_ref()).await {
                warn!("Failed to remove from index {}: {}", path.display(), e);
            }
        }
        _ => {}
    }
}

/// Index a single file
async fn index_file(
    path: &PathBuf,
    config: &Config,
    embedding_model: &mut EmbeddingModel,
    vectordb: &VectorDB,
) -> Result<()> {
    // Skip if not a file
    if !path.is_file() {
        return Ok(());
    }

    // Skip if not text-like content
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    if mime.type_() != mime::TEXT && !is_code_file(path) {
        return Ok(());
    }

    // Skip if file too large
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > config.indexing.max_file_size {
        return Ok(());
    }

    // Read content
    let content = std::fs::read_to_string(path)?;

    // Generate embedding
    let embedding = embedding_model.embed_text(&content)?;

    // Get metadata
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

    debug!("Indexed: {}", path.display());
    Ok(())
}
