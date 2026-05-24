//! Tier-aware salience adjustment for retrieval results (P8).
//!
//! Applied AFTER RRF + reranking. Multiplies each result's confidence by
//! the tier_factor() for its article's current tier. Archive items are
//! excluded unless `include_archive=true` in the retrieval config.

use anyhow::Result;

use crate::k2k::models::K2KResult;
use crate::maintenance::decay::tier_factor;
use crate::store::Store;

/// Apply tier-aware salience weighting to a result set. Mutates each
/// result's confidence in place; drops Archive items if `include_archive=false`.
///
/// Returns the filtered + weighted Vec.
pub async fn apply_tier_weighting(
    db: &dyn Store,
    results: Vec<K2KResult>,
    include_archive: bool,
) -> Result<Vec<K2KResult>> {
    let mut out = Vec::with_capacity(results.len());
    for mut r in results {
        let article = match db.get_article(&r.article_id).await {
            Ok(Some(a)) => a,
            _ => {
                // Article missing — keep result as-is (defensive)
                out.push(r);
                continue;
            }
        };
        let factor = tier_factor(article.tier, include_archive);
        if factor == 0.0 {
            // Archive-tier with include_archive=false → exclude
            continue;
        }
        r.confidence *= factor;
        // Reflect tier in result metadata for client-side filtering
        if let serde_json::Value::Object(ref mut map) = r.metadata {
            map.insert("tier".into(),
                serde_json::Value::String(
                    crate::maintenance::decay::tier_label(article.tier).into()
                )
            );
        }
        out.push(r);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Article, SurrealStore, Tier};

    fn fresh_article(id: &str, tier: Tier) -> Article {
        Article {
            id: id.into(),
            store_id: "tw-s1".into(),
            title: format!("T-{}", id),
            content: "C".into(),
            source_type: "user".into(),
            source_id: String::new(),
            content_hash: format!("{}-h", id),
            tags: serde_json::json!([]),
            embedded_at: None,
            created_at: "2026-05-24T00:00:00Z".into(),
            updated_at: "2026-05-24T00:00:00Z".into(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: "2026-05-24T00:00:00Z".into(),
            importance_score: 0.5,
            tier,
            pinned: false,
            compacted_into: None,
        }
    }

    fn make_result(id: &str, confidence: f32) -> K2KResult {
        use crate::k2k::models::ResultProvenance;
        K2KResult {
            article_id: id.into(),
            store_id: "tw-s1".into(),
            title: format!("T-{}", id),
            summary: String::new(),
            content: String::new(),
            confidence,
            source_type: "local".into(),
            tags: vec![],
            metadata: serde_json::json!({}),
            provenance: Some(ResultProvenance {
                store_id: "tw-s1".into(),
                store_type: "test".into(),
                original_rank: 0,
                rrf_score: 0.0,
            }),
        }
    }

    #[tokio::test]
    async fn hot_tier_keeps_full_confidence_warm_dampens() {
        let store = SurrealStore::open_in_memory().await.unwrap();
        store.create_article(&fresh_article("tw-hot", Tier::Hot)).await.unwrap();
        store.create_article(&fresh_article("tw-warm", Tier::Warm)).await.unwrap();

        let results = vec![
            make_result("tw-hot", 0.8),
            make_result("tw-warm", 0.8),
        ];

        let weighted = apply_tier_weighting(&store, results, false).await.unwrap();
        assert_eq!(weighted.len(), 2);

        let hot = weighted.iter().find(|r| r.article_id == "tw-hot").unwrap();
        let warm = weighted.iter().find(|r| r.article_id == "tw-warm").unwrap();
        assert!((hot.confidence - 0.8).abs() < 1e-6, "Hot should retain confidence; got {}", hot.confidence);
        assert!((warm.confidence - 0.4).abs() < 1e-6, "Warm should be 0.5×; got {}", warm.confidence);
    }

    #[tokio::test]
    async fn archive_excluded_by_default_included_with_flag() {
        let store = SurrealStore::open_in_memory().await.unwrap();
        store.create_article(&fresh_article("tw-arch", Tier::Archive)).await.unwrap();
        store.create_article(&fresh_article("tw-hot2", Tier::Hot)).await.unwrap();

        let results = vec![
            make_result("tw-arch", 0.7),
            make_result("tw-hot2", 0.7),
        ];

        let excluded = apply_tier_weighting(&store, results.clone(), false).await.unwrap();
        assert_eq!(excluded.len(), 1, "default should exclude Archive");
        assert_eq!(excluded[0].article_id, "tw-hot2");

        let included = apply_tier_weighting(&store, results, true).await.unwrap();
        assert_eq!(included.len(), 2, "include_archive=true should keep Archive");
        let arch = included.iter().find(|r| r.article_id == "tw-arch").unwrap();
        assert!(arch.confidence < 0.1, "Archive items should be heavily dampened; got {}", arch.confidence);
    }

    #[tokio::test]
    async fn tier_added_to_metadata() {
        let store = SurrealStore::open_in_memory().await.unwrap();
        store.create_article(&fresh_article("tw-meta", Tier::Cold)).await.unwrap();

        let results = vec![make_result("tw-meta", 0.5)];
        let weighted = apply_tier_weighting(&store, results, false).await.unwrap();

        let tier_meta = weighted[0].metadata.get("tier").and_then(|v| v.as_str());
        assert_eq!(tier_meta, Some("cold"),
            "result metadata should carry the tier label; got {:?}", weighted[0].metadata);
    }
}
