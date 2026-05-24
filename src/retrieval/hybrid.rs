//! Hybrid Searcher - combines LanceDB vector search with SQLite FTS5 keyword search,
//! merging results with Reciprocal Rank Fusion.

use std::sync::Arc;

use anyhow::Result;

use crate::store::Store;
use crate::k2k::models::{K2KResult, ResultProvenance};

const RRF_K: f32 = 60.0;

/// A ranked list of results from a single retrieval signal (vector, FTS, graph)
/// with an associated weight for RRF fusion.
pub struct RankedSignal {
    pub results: Vec<K2KResult>,
    pub weight: f32,
}

/// Merge N ranked result lists using weighted Reciprocal Rank Fusion.
///
/// Each signal contributes `weight / (rrf_k + rank + 1)` per result.
/// Articles appearing in multiple signals accumulate scores.
pub fn merge_signals(
    signals: Vec<RankedSignal>,
    top_k: usize,
    rrf_k: f32,
) -> Vec<K2KResult> {
    use std::collections::HashMap;

    let mut scores: HashMap<String, (f32, K2KResult)> = HashMap::new();

    for signal in signals {
        for (rank, result) in signal.results.into_iter().enumerate() {
            let rrf_score = signal.weight / (rrf_k + rank as f32 + 1.0);
            let key = result.article_id.clone();
            let entry = scores.entry(key).or_insert_with(|| (0.0, result.clone()));
            entry.0 += rrf_score;
            if result.confidence > entry.1.confidence {
                entry.1 = result;
            }
        }
    }

    let mut results: Vec<(f32, K2KResult)> = scores.into_values().collect();
    results.sort_by(|a, b| {
        match b.0.partial_cmp(&a.0) {
            Some(ord) => ord,
            None => {
                if b.0.is_nan() { std::cmp::Ordering::Less }
                else { std::cmp::Ordering::Greater }
            }
        }
    });

    results
        .into_iter()
        .take(top_k)
        .map(|(score, mut result)| {
            result.confidence = score;
            if let Some(ref mut prov) = result.provenance {
                prov.rrf_score = score;
            }
            result
        })
        .collect()
}

pub struct HybridSearcher {
    db: Arc<dyn Store>,
}

impl HybridSearcher {
    pub fn new(db: Arc<dyn Store>) -> Self {
        Self { db }
    }

    /// Perform FTS keyword search on articles
    pub async fn keyword_search(&self, query: &str, limit: usize) -> Result<Vec<K2KResult>> {
        let results = self.db.fts_search_articles(query, limit).await?;
        Ok(results
            .into_iter()
            .enumerate()
            .map(|(rank, article)| K2KResult {
                article_id: article.id.clone(),
                store_id: article.store_id.clone(),
                title: article.title.clone(),
                summary: if article.content.len() > 200 {
                    let end = (0..=200)
                        .rev()
                        .find(|&i| article.content.is_char_boundary(i))
                        .unwrap_or(0);
                    format!("{}...", &article.content[..end])
                } else {
                    article.content.clone()
                },
                content: article.content,
                confidence: 1.0 / (RRF_K + rank as f32 + 1.0), // RRF-style scoring
                source_type: article.source_type,
                tags: serde_json::from_value(article.tags).unwrap_or_else(|e| {
                    tracing::warn!("Failed to deserialize article tags: {}", e);
                    vec![]
                }),
                metadata: serde_json::json!({
                    "search_type": "keyword",
                    "fts_rank": rank,
                }),
                provenance: Some(ResultProvenance {
                    store_id: article.store_id,
                    store_type: "fts5".into(),
                    original_rank: rank,
                    rrf_score: 0.0,
                }),
            })
            .collect())
    }

    /// Merge vector search results with keyword search results using RRF
    pub fn merge_hybrid(
        &self,
        vector_results: Vec<K2KResult>,
        keyword_results: Vec<K2KResult>,
        top_k: usize,
    ) -> Vec<K2KResult> {
        let signals = vec![
            RankedSignal { results: vector_results, weight: 1.0 },
            RankedSignal { results: keyword_results, weight: 1.1 },
        ];
        merge_signals(signals, top_k, RRF_K)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::k2k::models::ResultProvenance;

    fn make_result(id: &str, store_id: &str) -> K2KResult {
        K2KResult {
            article_id: id.into(),
            store_id: store_id.into(),
            title: format!("Article {}", id),
            summary: String::new(),
            content: String::new(),
            confidence: 0.0,
            source_type: "local".into(),
            tags: vec![],
            metadata: serde_json::json!({}),
            provenance: Some(ResultProvenance {
                store_id: store_id.into(),
                store_type: "test".into(),
                original_rank: 0,
                rrf_score: 0.0,
            }),
        }
    }

    #[test]
    fn test_merge_signals_two_lists() {
        let vector = vec![make_result("a1", "s1"), make_result("a2", "s1")];
        let keyword = vec![make_result("a2", "s1"), make_result("a3", "s1")];
        let signals = vec![
            RankedSignal { results: vector, weight: 1.0 },
            RankedSignal { results: keyword, weight: 1.1 },
        ];
        let merged = merge_signals(signals, 10, 60.0);
        // a2 appears in both, should rank highest
        assert_eq!(merged[0].article_id, "a2");
        assert_eq!(merged.len(), 3);
    }

    #[test]
    fn test_merge_signals_three_lists_with_graph() {
        let vector = vec![make_result("a1", "s1"), make_result("a2", "s1")];
        let keyword = vec![make_result("a2", "s1"), make_result("a3", "s1")];
        let graph = vec![make_result("a3", "s1"), make_result("a4", "s1")];
        let signals = vec![
            RankedSignal { results: vector, weight: 1.0 },
            RankedSignal { results: keyword, weight: 1.1 },
            RankedSignal { results: graph, weight: 0.8 },
        ];
        let merged = merge_signals(signals, 10, 60.0);
        assert_eq!(merged.len(), 4); // a1, a2, a3, a4
        // a2 and a3 appear in 2 lists each, should rank above a1 and a4
        let top_two: Vec<&str> = merged[..2].iter().map(|r| r.article_id.as_str()).collect();
        assert!(top_two.contains(&"a2"));
        assert!(top_two.contains(&"a3"));
    }

    #[test]
    fn test_merge_signals_zero_weight_list_ignored() {
        let vector = vec![make_result("a1", "s1")];
        let graph = vec![make_result("a2", "s1")];
        let signals = vec![
            RankedSignal { results: vector, weight: 1.0 },
            RankedSignal { results: graph, weight: 0.0 },
        ];
        let merged = merge_signals(signals, 10, 60.0);
        // a2 should still appear but with 0 score from graph
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].article_id, "a1");
    }

    #[test]
    fn test_merge_signals_empty_inputs() {
        let signals: Vec<RankedSignal> = vec![];
        let merged = merge_signals(signals, 10, 60.0);
        assert!(merged.is_empty());
    }

    #[test]
    fn test_merge_signals_top_k_limit() {
        let long_list: Vec<K2KResult> = (0..20).map(|i| make_result(&format!("a{}", i), "s1")).collect();
        let signals = vec![RankedSignal { results: long_list, weight: 1.0 }];
        let merged = merge_signals(signals, 5, 60.0);
        assert_eq!(merged.len(), 5);
    }
}
