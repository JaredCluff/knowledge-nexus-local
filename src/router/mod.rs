pub mod context;
pub mod executor;
pub mod merger;
pub mod planner;

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::Mutex;
use tracing::{debug, info};

use crate::store::Store;
use crate::embeddings::EmbeddingModel;
use crate::federation::RemoteQueryExecutor;
use crate::k2k::models::K2KQueryResponse;
use crate::retrieval::{ConfidenceScorer, HybridSearcher, QueryExpander, Reranker};
use crate::vectordb::VectorDB;

use self::context::ContextClassifier;
use self::executor::QueryExecutor;
use self::merger::ResultMerger;
use self::planner::QueryPlanner;

pub struct LocalRouter {
    classifier: ContextClassifier,
    planner: QueryPlanner,
    executor: QueryExecutor,
    merger: ResultMerger,
    reranker: Reranker,
    query_expander: QueryExpander,
    confidence_scorer: ConfidenceScorer,
    remote_executor: Option<Arc<RemoteQueryExecutor>>,
}

impl LocalRouter {
    pub fn new(
        db: Arc<dyn Store>,
        vectordb: Arc<VectorDB>,
        embedding_model: Arc<Mutex<EmbeddingModel>>,
        hybrid_searcher: Option<Arc<HybridSearcher>>,
        remote_executor: Option<Arc<RemoteQueryExecutor>>,
    ) -> Self {
        Self {
            classifier: ContextClassifier::new(),
            planner: QueryPlanner::new(db.clone()),
            executor: QueryExecutor::new(vectordb, embedding_model, hybrid_searcher),
            merger: ResultMerger::new(),
            reranker: Reranker::new(),
            query_expander: QueryExpander::new(),
            confidence_scorer: ConfidenceScorer::new(),
            remote_executor,
        }
    }

    pub async fn route(
        &self,
        query: &str,
        user_id: &str,
        hint: Option<&str>,
        top_k: usize,
    ) -> Result<K2KQueryResponse> {
        let start = std::time::Instant::now();

        // 1. Classify context
        let scope = hint
            .map(|h| h.to_string())
            .unwrap_or_else(|| self.classifier.classify(query));

        // 2. Plan which stores to query
        let stores = self.planner.plan(user_id, &scope).await?;
        let store_names: Vec<String> = stores.iter().map(|s| s.name.clone()).collect();
        let store_collections: Vec<(String, String, String)> = stores
            .iter()
            .map(|s| {
                (
                    s.id.clone(),
                    s.store_type.clone(),
                    s.lancedb_collection.clone(),
                )
            })
            .collect();

        // 3. Execute queries across stores (hybrid: vector + keyword)
        let mut store_results = self
            .executor
            .execute(query, &store_collections, top_k)
            .await?;

        // 3b. Federation: query remote nodes if available
        if let Some(ref remote) = self.remote_executor {
            match remote.query_remote_nodes(query, top_k).await {
                Ok(remote_results) if !remote_results.is_empty() => {
                    debug!(
                        "Federation: got results from {} remote nodes",
                        remote_results.len()
                    );
                    store_results.extend(remote_results);
                }
                Ok(_) => {}
                Err(e) => {
                    debug!("Federation query failed (local results only): {}", e);
                }
            }
        }

        // 4. Merge results with RRF
        let merged = self.merger.merge_rrf(store_results, top_k);

        // 5. Rerank: boost by title/phrase/recency
        let reranked = self.reranker.rerank(merged, query);

        // 6. Confidence scoring + query expansion retry
        let final_results = if !reranked.is_empty()
            && self.confidence_scorer.all_low_confidence(&reranked, query)
        {
            debug!("All results are low confidence, attempting query expansion");
            let variants = self.query_expander.expand(query);

            // Try the first non-original variant
            if let Some(expanded_query) = variants.into_iter().nth(1) {
                info!("Retrying with expanded query: \"{}\"", expanded_query);
                let retry_results = self
                    .executor
                    .execute(&expanded_query, &store_collections, top_k)
                    .await?;
                let retry_merged = self.merger.merge_rrf(retry_results, top_k);
                let retry_reranked = self.reranker.rerank(retry_merged, &expanded_query);

                // Use expanded results if they're better
                if !retry_reranked.is_empty()
                    && !self
                        .confidence_scorer
                        .all_low_confidence(&retry_reranked, &expanded_query)
                {
                    retry_reranked
                } else {
                    reranked
                }
            } else {
                reranked
            }
        } else {
            reranked
        };

        let query_time_ms = start.elapsed().as_millis() as u64;

        Ok(K2KQueryResponse {
            query_id: uuid::Uuid::new_v4().to_string(),
            total_results: final_results.len(),
            results: final_results,
            stores_queried: store_names,
            query_time_ms,
            routing_decision: Some(serde_json::json!({
                "scope": scope,
                "stores_selected": store_collections.len(),
                "federation_enabled": self.remote_executor.is_some(),
            })),
            trace_id: None,
        })
    }
}
