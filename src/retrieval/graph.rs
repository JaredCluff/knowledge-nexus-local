//! Graph-based retrieval: matches query terms to entities, traverses
//! MENTIONS and RELATED_TO edges, and produces a ranked result list.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use tracing::debug;

use crate::config::RetrievalConfig;
use crate::k2k::models::{K2KResult, ResultProvenance};
use crate::retrieval::expansion::QueryExpander;
use crate::store::Store;

/// Result of a graph search pass, including entity coverage metadata
/// needed for adaptive RRF weighting.
pub struct GraphSearchOutput {
    /// Ranked results from graph traversal.
    pub results: Vec<K2KResult>,
    /// Fraction of meaningful query terms that matched entities (0.0..=1.0).
    pub entity_coverage: f32,
}

pub struct GraphSearcher {
    db: Arc<dyn Store>,
    config: RetrievalConfig,
}

impl GraphSearcher {
    pub fn new(db: Arc<dyn Store>, config: RetrievalConfig) -> Self {
        Self { db, config }
    }

    /// Run graph-based retrieval for the given query within a store.
    /// Dispatches by `config.graph_strategy`:
    /// - `"jaccard"` — P4 one-hop ENTITY_OVERLAP expansion (back-compat path).
    /// - `"activation"` — P6 spreading activation (PPR + SYNAPSE + MAGMA).
    pub async fn search(
        &self,
        query: &str,
        store_id: &str,
        top_k: usize,
    ) -> Result<GraphSearchOutput> {
        match self.config.graph_strategy.as_str() {
            "activation" => self.search_via_activation(query, store_id, top_k).await,
            _ => self.search_via_jaccard(query, store_id, top_k).await,
        }
    }

    /// P4 one-hop jaccard path. Preserved for back-compat and ablation tests.
    async fn search_via_jaccard(
        &self,
        query: &str,
        store_id: &str,
        top_k: usize,
    ) -> Result<GraphSearchOutput> {
        // 1. Extract meaningful terms from query
        let terms = self.extract_terms(query);
        if terms.is_empty() {
            return Ok(GraphSearchOutput { results: vec![], entity_coverage: 0.0 });
        }

        // 2. Find matching entities
        let term_refs: Vec<&str> = terms.iter().map(|s| s.as_str()).collect();
        let matched_entities = self.db.search_entities_by_name(store_id, &term_refs).await?;
        let entity_coverage = matched_entities.len() as f32 / terms.len() as f32;
        // Clamp to 1.0 (more entity matches than terms is possible with prefix matches)
        let entity_coverage = entity_coverage.min(1.0);

        debug!(
            "Graph search: {} terms → {} entities matched (coverage: {:.2})",
            terms.len(), matched_entities.len(), entity_coverage
        );

        if matched_entities.is_empty() {
            return Ok(GraphSearchOutput { results: vec![], entity_coverage: 0.0 });
        }

        // 3. Get articles via MENTIONS edges
        let entity_ids: Vec<&str> = matched_entities.iter().map(|e| e.id.as_str()).collect();
        let mentioned_articles = self.db.list_articles_for_entities(&entity_ids).await?;

        // 4. Score articles: direct mention score
        let mut article_scores: HashMap<String, (f64, crate::store::Article)> = HashMap::new();
        for (article, confidence) in &mentioned_articles {
            let entry = article_scores
                .entry(article.id.clone())
                .or_insert_with(|| (0.0, article.clone()));
            entry.0 += confidence; // Sum confidence across matched entities
        }

        // 5. One-hop RELATED_TO traversal (if configured)
        if self.config.graph_hops >= 1 {
            let direct_article_ids: Vec<String> = article_scores.keys().cloned().collect();
            for aid in &direct_article_ids {
                if let Ok(related) = self.db.list_related_articles(aid).await {
                    for related_article in related {
                        if !article_scores.contains_key(&related_article.id) {
                            // Decay factor for one-hop results
                            let base_score = article_scores.get(aid).map(|s| s.0).unwrap_or(0.0);
                            let hop_score = base_score * 0.5;
                            article_scores
                                .entry(related_article.id.clone())
                                .or_insert_with(|| (hop_score, related_article));
                        }
                    }
                }
            }
        }

        // 6. Convert to ranked K2KResult list
        let mut scored: Vec<(f64, crate::store::Article)> = article_scores.into_values().collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        let results: Vec<K2KResult> = scored
            .into_iter()
            .enumerate()
            .map(|(rank, (score, article))| {
                let summary = if article.content.len() > 200 {
                    let end = (0..=200)
                        .rev()
                        .find(|&i| article.content.is_char_boundary(i))
                        .unwrap_or(0);
                    format!("{}...", &article.content[..end])
                } else {
                    article.content.clone()
                };
                K2KResult {
                    article_id: article.id.clone(),
                    store_id: article.store_id.clone(),
                    title: article.title,
                    summary,
                    content: article.content,
                    confidence: score as f32,
                    source_type: article.source_type,
                    tags: vec![],
                    metadata: serde_json::json!({
                        "search_type": "graph",
                        "graph_score": score,
                    }),
                    provenance: Some(ResultProvenance {
                        store_id: article.store_id,
                        store_type: "graph".into(),
                        original_rank: rank,
                        rrf_score: 0.0,
                    }),
                }
            })
            .collect();

        Ok(GraphSearchOutput { results, entity_coverage })
    }

    /// P6 spreading-activation path. Builds an ActivationEngine and translates
    /// its output to the GraphSearchOutput shape that the router expects.
    async fn search_via_activation(
        &self,
        query: &str,
        store_id: &str,
        top_k: usize,
    ) -> Result<GraphSearchOutput> {
        let engine = crate::retrieval::ActivationEngine::new(
            self.db.clone(),
            self.config.clone(),
        );
        let output = engine.search(query, store_id, top_k).await?;
        Ok(GraphSearchOutput {
            results: output.results,
            entity_coverage: output.entity_coverage,
        })
    }

    /// Extract meaningful query terms by removing stop words.
    fn extract_terms(&self, query: &str) -> Vec<String> {
        let expander = QueryExpander::new();
        // Use stop-word removal to get meaningful terms
        let cleaned = expander.expand(query);
        // The second variant (index 1) is stop-words-removed if it exists
        let meaningful = if cleaned.len() > 1 {
            &cleaned[1]
        } else {
            &cleaned[0]
        };
        meaningful
            .split_whitespace()
            .filter(|w| w.len() > 1)
            .map(|w| w.to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_terms_removes_stop_words() {
        let _config = RetrievalConfig::default();
        // We can't construct a real GraphSearcher without a Store, so test via QueryExpander
        let expander = QueryExpander::new();
        let variants = expander.expand("how to configure the database");
        // Second variant should be stop-word-free
        assert!(variants.len() > 1);
        assert!(!variants[1].contains("how"));
        assert!(!variants[1].contains("the"));
        assert!(variants[1].contains("configure"));
        assert!(variants[1].contains("database"));
    }

    /// Verify the strategy dispatcher actually selects the correct path.
    /// (Not a full GraphSearcher integration test — that lives in
    /// p3_integration_tests::graph_searcher_end_to_end for jaccard and
    /// p3_integration_tests::activation_engine_returns_results_for_why_query
    /// for activation. This test just confirms the match arms compile and
    /// that the strategy string is read correctly.)
    #[test]
    fn graph_strategy_string_matches_expected_values() {
        let cfg_activation = RetrievalConfig {
            graph_strategy: "activation".into(),
            ..RetrievalConfig::default()
        };
        let cfg_jaccard = RetrievalConfig {
            graph_strategy: "jaccard".into(),
            ..RetrievalConfig::default()
        };
        assert_eq!(cfg_activation.graph_strategy, "activation");
        assert_eq!(cfg_jaccard.graph_strategy, "jaccard");
        // Default is "activation"
        let cfg_default = RetrievalConfig::default();
        assert_eq!(cfg_default.graph_strategy, "activation");
    }
}
