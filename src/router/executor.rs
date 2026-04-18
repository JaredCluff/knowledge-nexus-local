use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;
use tracing::debug;

use crate::config::RetrievalConfig;
use crate::embeddings::EmbeddingModel;
use crate::k2k::models::{K2KResult, ResultProvenance};
use crate::retrieval::{HybridSearcher, GraphSearcher};
use crate::retrieval::hybrid::{RankedSignal, merge_signals};
use crate::vectordb::VectorDB;

/// Results from a single store search
#[allow(dead_code)]
pub struct StoreSearchResult {
    pub store_id: String,
    pub store_type: String,
    pub results: Vec<K2KResult>,
}

pub struct QueryExecutor {
    vectordb: Arc<VectorDB>,
    embedding_model: Arc<Mutex<EmbeddingModel>>,
    hybrid_searcher: Option<Arc<HybridSearcher>>,
    graph_searcher: Option<Arc<GraphSearcher>>,
    retrieval_config: RetrievalConfig,
}

impl QueryExecutor {
    pub fn new(
        vectordb: Arc<VectorDB>,
        embedding_model: Arc<Mutex<EmbeddingModel>>,
        hybrid_searcher: Option<Arc<HybridSearcher>>,
        graph_searcher: Option<Arc<GraphSearcher>>,
        retrieval_config: RetrievalConfig,
    ) -> Self {
        Self {
            vectordb,
            embedding_model,
            hybrid_searcher,
            graph_searcher,
            retrieval_config,
        }
    }

    /// Execute searches across multiple store collections
    pub async fn execute(
        &self,
        query: &str,
        stores: &[(String, String, String)], // (store_id, store_type, collection_name)
        top_k: usize,
    ) -> Result<Vec<StoreSearchResult>> {
        // Generate embedding once for all stores
        let embedding = {
            let mut model = self.embedding_model.lock().await;
            model.embed_text(query)?
        };

        let mut all_results = Vec::new();

        for (store_id, store_type, _collection) in stores {
            // Vector search
            let search_results = self.vectordb.search(&embedding, top_k).await?;

            let vector_results: Vec<K2KResult> = search_results
                .into_iter()
                .enumerate()
                .map(|(rank, r)| K2KResult {
                    article_id: r.id.clone(),
                    store_id: store_id.clone(),
                    title: r.title.clone(),
                    summary: r
                        .chunk_text
                        .as_ref()
                        .map(|t| {
                            if t.len() > 200 {
                                let end = (0..=200)
                                    .rev()
                                    .find(|&i| t.is_char_boundary(i))
                                    .unwrap_or(0);
                                format!("{}...", &t[..end])
                            } else {
                                t.clone()
                            }
                        })
                        .unwrap_or_default(),
                    content: r.chunk_text.unwrap_or_default(),
                    confidence: r.score,
                    source_type: r.source_type.clone(),
                    tags: vec![],
                    metadata: serde_json::json!({
                        "path": r.path,
                        "size_bytes": r.size_bytes,
                        "modified_at": r.modified_at,
                        "content_type": r.content_type,
                        "document_type": r.document_type,
                        "chunk_index": r.chunk_index,
                    }),
                    provenance: Some(ResultProvenance {
                        store_id: store_id.clone(),
                        store_type: store_type.clone(),
                        original_rank: rank,
                        rrf_score: 0.0,
                    }),
                })
                .collect();

            // Keyword search (if available)
            let keyword_results = if let Some(ref hybrid) = self.hybrid_searcher {
                match hybrid.keyword_search(query, top_k).await {
                    Ok(kw) if !kw.is_empty() => {
                        debug!(
                            "Hybrid search: {} vector + {} keyword results for store {}",
                            vector_results.len(), kw.len(), store_id
                        );
                        kw
                    }
                    Ok(_) => vec![],
                    Err(e) => {
                        debug!("Keyword search failed (using vector only): {}", e);
                        vec![]
                    }
                }
            } else {
                vec![]
            };

            // Graph search (if available)
            let (graph_results, entity_coverage) = if let Some(ref graph) = self.graph_searcher {
                match graph.search(query, store_id, top_k).await {
                    Ok(output) => {
                        debug!(
                            "Graph search: {} results, coverage {:.2} for store {}",
                            output.results.len(), output.entity_coverage, store_id
                        );
                        (output.results, output.entity_coverage)
                    }
                    Err(e) => {
                        debug!("Graph search failed (continuing without): {}", e);
                        (vec![], 0.0)
                    }
                }
            } else {
                (vec![], 0.0)
            };

            // Build signals for N-way RRF merge
            let cfg = &self.retrieval_config;
            let graph_weight = cfg.graph_weight_max * entity_coverage;
            let mut signals = vec![
                RankedSignal { results: vector_results, weight: cfg.vector_weight },
            ];
            if !keyword_results.is_empty() {
                signals.push(RankedSignal { results: keyword_results, weight: cfg.keyword_weight });
            }
            if !graph_results.is_empty() && graph_weight > 0.0 {
                signals.push(RankedSignal { results: graph_results, weight: graph_weight });
            }

            let final_results = merge_signals(signals, top_k, cfg.rrf_k);

            all_results.push(StoreSearchResult {
                store_id: store_id.clone(),
                store_type: store_type.clone(),
                results: final_results,
            });
        }

        Ok(all_results)
    }
}
