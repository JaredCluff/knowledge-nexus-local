use std::collections::HashMap;

use crate::k2k::models::{K2KResult, ResultProvenance};

use super::executor::StoreSearchResult;

const RRF_K: f32 = 60.0;

pub struct ResultMerger;

impl ResultMerger {
    pub fn new() -> Self {
        Self
    }

    /// Merge results from multiple stores using Reciprocal Rank Fusion
    pub fn merge_rrf(&self, store_results: Vec<StoreSearchResult>, top_k: usize) -> Vec<K2KResult> {
        // If only one store, just return its results
        if store_results.len() == 1 {
            // Safety: len() == 1 guarantees next() returns Some
            let mut results = store_results
                .into_iter()
                .next()
                .expect("checked len == 1")
                .results;
            results.truncate(top_k);
            return results;
        }

        // Collect all results with RRF scores
        // Key: article_id, Value: (rrf_score, K2KResult)
        let mut scored: HashMap<String, (f32, K2KResult)> = HashMap::new();

        for store_result in store_results {
            for (rank, mut result) in store_result.results.into_iter().enumerate() {
                let rrf_score = 1.0 / (RRF_K + rank as f32 + 1.0);

                let key = result.article_id.clone();
                let entry = scored.entry(key).or_insert_with(|| (0.0, result.clone()));

                entry.0 += rrf_score;

                // Update provenance with RRF score
                if let Some(ref mut prov) = result.provenance {
                    prov.rrf_score = entry.0;
                }

                // Keep the result with the highest individual confidence
                if result.confidence > entry.1.confidence {
                    let total_rrf = entry.0;
                    entry.1 = result;
                    entry.1.provenance = Some(ResultProvenance {
                        store_id: entry.1.store_id.clone(),
                        store_type: entry
                            .1
                            .provenance
                            .as_ref()
                            .map(|p| p.store_type.clone())
                            .unwrap_or_default(),
                        original_rank: rank,
                        rrf_score: total_rrf,
                    });
                }
            }
        }

        // Sort by RRF score descending
        let mut results: Vec<(f32, K2KResult)> = scored.into_values().collect();
        results.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        // Update provenance rrf_score and take top_k
        results
            .into_iter()
            .take(top_k)
            .map(|(rrf_score, mut result)| {
                if let Some(ref mut prov) = result.provenance {
                    prov.rrf_score = rrf_score;
                }
                result.confidence = rrf_score;
                result
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_result(id: &str, store_id: &str, store_type: &str, confidence: f32) -> K2KResult {
        K2KResult {
            article_id: id.to_string(),
            store_id: store_id.to_string(),
            title: format!("Article {}", id),
            summary: String::new(),
            content: String::new(),
            confidence,
            source_type: "local".to_string(),
            tags: vec![],
            metadata: serde_json::json!({}),
            provenance: Some(ResultProvenance {
                store_id: store_id.to_string(),
                store_type: store_type.to_string(),
                original_rank: 0,
                rrf_score: 0.0,
            }),
        }
    }

    #[test]
    fn test_single_store() {
        let merger = ResultMerger::new();
        let results = vec![StoreSearchResult {
            store_id: "s1".into(),
            store_type: "personal".into(),
            results: vec![
                make_result("a1", "s1", "personal", 0.9),
                make_result("a2", "s1", "personal", 0.8),
            ],
        }];
        let merged = merger.merge_rrf(results, 10);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].article_id, "a1");
    }

    #[test]
    fn test_multi_store_rrf() {
        let merger = ResultMerger::new();
        let results = vec![
            StoreSearchResult {
                store_id: "s1".into(),
                store_type: "personal".into(),
                results: vec![
                    make_result("a1", "s1", "personal", 0.9),
                    make_result("a2", "s1", "personal", 0.7),
                ],
            },
            StoreSearchResult {
                store_id: "s2".into(),
                store_type: "family".into(),
                results: vec![
                    make_result("a3", "s2", "family", 0.85),
                    make_result("a1", "s2", "family", 0.6), // Duplicate
                ],
            },
        ];
        let merged = merger.merge_rrf(results, 10);
        // a1 should be ranked highest (appears in both stores)
        assert_eq!(merged[0].article_id, "a1");
        assert!(merged[0].confidence > merged[1].confidence);
    }

    #[test]
    fn test_deduplication() {
        let merger = ResultMerger::new();
        let results = vec![
            StoreSearchResult {
                store_id: "s1".into(),
                store_type: "personal".into(),
                results: vec![make_result("a1", "s1", "personal", 0.9)],
            },
            StoreSearchResult {
                store_id: "s2".into(),
                store_type: "family".into(),
                results: vec![make_result("a1", "s2", "family", 0.8)],
            },
        ];
        let merged = merger.merge_rrf(results, 10);
        // Should be deduplicated to 1 result
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].article_id, "a1");
    }
}
