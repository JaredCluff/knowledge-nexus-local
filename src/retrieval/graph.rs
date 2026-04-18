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
    pub async fn search(
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
        let config = RetrievalConfig::default();
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
}
