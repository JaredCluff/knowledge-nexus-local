//! Hybrid Searcher - combines LanceDB vector search with SQLite FTS5 keyword search,
//! merging results with Reciprocal Rank Fusion.

use std::sync::Arc;

use anyhow::Result;

use crate::db::Database;
use crate::k2k::models::{K2KResult, ResultProvenance};

const RRF_K: f32 = 60.0;

pub struct HybridSearcher {
    db: Arc<Database>,
}

impl HybridSearcher {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Perform FTS5 keyword search on articles
    pub fn keyword_search(&self, query: &str, limit: usize) -> Result<Vec<K2KResult>> {
        let results = self.db.fts_search_articles(query, limit)?;
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
        use std::collections::HashMap;

        let mut scores: HashMap<String, (f32, K2KResult)> = HashMap::new();

        // Score vector results
        for (rank, result) in vector_results.into_iter().enumerate() {
            let rrf_score = 1.0 / (RRF_K + rank as f32 + 1.0);
            let key = result.article_id.clone();
            let entry = scores.entry(key).or_insert_with(|| (0.0, result.clone()));
            entry.0 += rrf_score;
            if result.confidence > entry.1.confidence {
                entry.1 = result;
            }
        }

        // Score keyword results (weighted slightly higher for exact matches)
        let keyword_weight = 1.1;
        for (rank, result) in keyword_results.into_iter().enumerate() {
            let rrf_score = keyword_weight / (RRF_K + rank as f32 + 1.0);
            let key = result.article_id.clone();
            let entry = scores.entry(key).or_insert_with(|| (0.0, result.clone()));
            entry.0 += rrf_score;
            if result.confidence > entry.1.confidence {
                entry.1 = result;
            }
        }

        // Sort by combined RRF score
        let mut results: Vec<(f32, K2KResult)> = scores.into_values().collect();
        results.sort_by(|a, b| {
            match b.0.partial_cmp(&a.0) {
                Some(ord) => ord,
                None => {
                    // NaN safety: treat NaN as lowest score
                    if b.0.is_nan() {
                        std::cmp::Ordering::Less
                    } else {
                        std::cmp::Ordering::Greater
                    }
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
}
