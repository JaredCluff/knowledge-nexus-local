//! HippoRAG-style node specificity: rare entities get higher weight.
//!
//! `s_i = 1 / max(1, |P_i|)` where P_i is the set of articles mentioning
//! entity i. Cached per-store via RwLock; lazy-init on first query.
//! Invalidate after writes that change the entity-mention graph.
//!
//! Per HippoRAG ablation (arXiv 2405.14831, Table 5): contributes
//! -2.7 to -3.8 pts F1 on MuSiQue/HotpotQA when removed.

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use anyhow::Result;

use crate::store::Store;

#[derive(Default)]
pub struct SpecificityCache {
    by_store: RwLock<HashMap<String, Arc<HashMap<String, f32>>>>,
}

impl SpecificityCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Get specificity weights for `store_id`. Computes on first call;
    /// reuses cached map on subsequent calls until `invalidate()` is called.
    pub async fn get<S: Store + ?Sized + Sync>(
        &self,
        store: &S,
        store_id: &str,
    ) -> Result<Arc<HashMap<String, f32>>> {
        // Fast path: cache hit
        if let Some(cached) = self.by_store.read().await.get(store_id) {
            return Ok(Arc::clone(cached));
        }

        // Compute fresh
        let counts = store.count_mentions_per_entity(store_id).await?;
        let weights: HashMap<String, f32> = counts.into_iter()
            .map(|(id, n)| {
                let n = n.max(1);
                (id, 1.0 / n as f32)
            })
            .collect();
        let arc = Arc::new(weights);

        // Insert into cache (someone may have raced us; last writer wins, both correct)
        self.by_store.write().await.insert(store_id.to_string(), Arc::clone(&arc));
        Ok(arc)
    }

    /// Drop the cached weights for a store. Call after writes that add
    /// or remove mentions edges.
    #[allow(dead_code)]
    pub async fn invalidate(&self, store_id: &str) {
        self.by_store.write().await.remove(store_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Entity, SurrealStore, Article};

    /// Build a fresh in-memory store with seeded entities + articles + mentions.
    async fn build_fixture() -> SurrealStore {
        let s = SurrealStore::open_in_memory().await.expect("open mem");
        let ts = "2026-05-24T00:00:00Z".to_string();

        // 4 articles
        for (id, title) in &[
            ("spec-a1", "A1"), ("spec-a2", "A2"), ("spec-a3", "A3"), ("spec-a4", "A4"),
        ] {
            s.create_article(&Article {
                id: id.to_string(), store_id: "spec-s1".into(),
                title: title.to_string(), content: "".into(),
                source_type: "user".into(), source_id: String::new(),
                content_hash: format!("{}-h", id), tags: serde_json::json!([]),
                embedded_at: None,
                created_at: ts.clone(), updated_at: ts.clone(),
                reflects: vec![],
            }).await.unwrap();
        }

        // 2 entities: "common" mentioned in 4 articles, "rare" in 1
        s.create_entity(&Entity {
            id: "spec-ent-common".into(), name: "common".into(),
            entity_type: "concept".into(), description: None,
            store_id: "spec-s1".into(), mention_count: 4,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_entity(&Entity {
            id: "spec-ent-rare".into(), name: "rare".into(),
            entity_type: "concept".into(), description: None,
            store_id: "spec-s1".into(), mention_count: 1,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        // MENTIONS edges
        for aid in &["spec-a1", "spec-a2", "spec-a3", "spec-a4"] {
            s.create_mentions_edge(aid, "spec-ent-common", "common", 0.9).await.unwrap();
        }
        s.create_mentions_edge("spec-a1", "spec-ent-rare", "rare mention", 0.95).await.unwrap();

        s
    }

    #[tokio::test]
    async fn specificity_inverse_proportional_to_mention_count() {
        let store = build_fixture().await;
        let cache = SpecificityCache::new();
        let weights = cache.get(&store, "spec-s1").await.expect("get");

        let common_w = weights.get("spec-ent-common").copied().unwrap_or(0.0);
        let rare_w = weights.get("spec-ent-rare").copied().unwrap_or(0.0);

        // Common: 4 articles → 1/4 = 0.25
        // Rare: 1 article → 1/1 = 1.0
        assert!((common_w - 0.25).abs() < 1e-6,
            "common entity expected 0.25, got {}", common_w);
        assert!((rare_w - 1.0).abs() < 1e-6,
            "rare entity expected 1.0, got {}", rare_w);
        assert!(rare_w > common_w, "rare must outweigh common");
    }

    #[tokio::test]
    async fn specificity_cache_hits_after_first_load() {
        let store = build_fixture().await;
        let cache = SpecificityCache::new();

        // First load
        let w1 = cache.get(&store, "spec-s1").await.expect("first get");

        // Second load: same Arc returned (cache hit)
        let w2 = cache.get(&store, "spec-s1").await.expect("second get");

        // Both Arcs point to the same underlying HashMap (test via Arc::ptr_eq)
        assert!(Arc::ptr_eq(&w1, &w2),
            "second get should return the cached Arc, not a fresh computation");

        // After invalidate, the next get returns a NEW Arc (cache miss)
        cache.invalidate("spec-s1").await;
        let w3 = cache.get(&store, "spec-s1").await.expect("third get");
        assert!(!Arc::ptr_eq(&w1, &w3),
            "after invalidate, get should produce a fresh Arc");
    }
}
