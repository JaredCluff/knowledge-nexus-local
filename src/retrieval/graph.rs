//! Graph-based retrieval: matches query terms to entities, traverses
//! MENTIONS and RELATED_TO edges, and produces a ranked result list.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use tracing::debug;

use crate::config::RetrievalConfig;
use crate::k2k::models::{K2KResult, ResultProvenance};
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
        let terms = extract_terms(query);
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

        // 5. One-hop RELATED_TO traversal (if enabled)
        if self.config.graph_hop_enabled {
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
}

/// Extract meaningful query terms by removing stop words.
fn extract_terms(query: &str) -> Vec<String> {
    const STOP_WORDS: &[&str] = &[
        "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "have", "has",
        "had", "do", "does", "did", "will", "would", "could", "should", "may", "might", "can",
        "shall", "to", "of", "in", "for", "on", "with", "at", "by", "from", "as", "into",
        "about", "like", "through", "after", "over", "between", "out", "against", "during",
        "without", "before", "under", "around", "among", "it", "its", "this", "that", "these",
        "those", "my", "your", "his", "her", "how", "what", "when", "where", "who", "which",
        "why",
    ];
    query
        .split_whitespace()
        .filter(|w| w.len() > 1 && !STOP_WORDS.contains(&w.to_lowercase().as_str()))
        .map(|w| w.to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_terms_removes_stop_words() {
        let terms = extract_terms("how to configure the database");
        assert_eq!(terms, vec!["configure", "database"]);
    }

    #[test]
    fn test_extract_terms_filters_single_char() {
        let terms = extract_terms("I want a Rust guide");
        assert!(!terms.contains(&"i".to_string()));
        assert!(!terms.contains(&"a".to_string()));
        assert!(terms.contains(&"rust".to_string()));
        assert!(terms.contains(&"guide".to_string()));
    }

    #[test]
    fn test_extract_terms_lowercases() {
        let terms = extract_terms("Tokio Runtime");
        assert_eq!(terms, vec!["tokio", "runtime"]);
    }
}
