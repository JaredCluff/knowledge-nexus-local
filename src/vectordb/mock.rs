//! Test-only mock VectorDb. Returns pre-seeded neighbor pairs.
//!
//! Encodes the article_id into the embedding returned by `get_embedding` so
//! that `ann_query` can decode it and look up the pre-seeded neighbor list —
//! without ever touching the file-system LanceDB.

#![cfg(test)]

use anyhow::Result;
use std::collections::HashMap;

use crate::vectordb::VectorDbBackfillApi;

pub struct MockVectorDb {
    /// Map<store_id, Map<article_id, Vec<(neighbor_id, similarity)>>>
    pairs: HashMap<String, HashMap<String, Vec<(String, f64)>>>,
}

impl MockVectorDb {
    pub fn with_pairs(store_id: &str, data: &[(&str, &[(&str, f64)])]) -> Self {
        let mut store = HashMap::new();
        for (article_id, neighbors) in data {
            store.insert(
                article_id.to_string(),
                neighbors.iter().map(|(n, s)| (n.to_string(), *s)).collect(),
            );
        }
        let mut pairs = HashMap::new();
        pairs.insert(store_id.to_string(), store);
        Self { pairs }
    }

    fn neighbors_for(&self, store_id: &str, article_id: &str) -> Vec<(String, f64)> {
        self.pairs
            .get(store_id)
            .and_then(|m| m.get(article_id))
            .cloned()
            .unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl VectorDbBackfillApi for MockVectorDb {
    async fn get_embedding(&self, store_id: &str, article_id: &str) -> Result<Option<Vec<f32>>> {
        if self.pairs.get(store_id).and_then(|m| m.get(article_id)).is_some() {
            // Encode the article_id into the embedding so ann_query can recover it.
            Ok(Some(article_id.bytes().map(|b| b as f32).collect()))
        } else {
            Ok(None)
        }
    }

    async fn ann_query(
        &self,
        store_id: &str,
        query_embedding: &[f32],
        _top_k: usize,
    ) -> Result<Vec<(String, f64)>> {
        // Decode the article_id we encoded in get_embedding.
        let article_id: String = query_embedding.iter().map(|&f| f as u8 as char).collect();
        Ok(self.neighbors_for(store_id, &article_id))
    }
}
