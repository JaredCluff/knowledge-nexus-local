//! Deterministic temporal-edge backfill.
//!
//! For each ENTITY_OVERLAP cluster, emit PRECEDES edges in `created_at` order.
//! No LLM cost; runs on the existing P3 entity-overlap graph.

use anyhow::Result;
use chrono::DateTime;

use crate::store::Store;

/// Per-store temporal backfill. Returns the number of PRECEDES edges created.
pub async fn backfill_temporal<S: Store + Sync + ?Sized>(store: &S, store_id: &str) -> Result<u64> {
    let pairs = store.list_entity_overlap_pairs(store_id).await?;
    let mut count = 0u64;

    for (a_id, b_id) in pairs {
        let a = store.get_article(&a_id).await?;
        let b = store.get_article(&b_id).await?;
        let (Some(a), Some(b)) = (a, b) else { continue };

        let a_t = parse_ts(&a.created_at);
        let b_t = parse_ts(&b.created_at);
        let (Some(a_t), Some(b_t)) = (a_t, b_t) else { continue };

        let (from, to) = if a_t < b_t {
            (a_id, b_id)
        } else if b_t < a_t {
            (b_id, a_id)
        } else {
            continue;
        };

        store.create_precedes_edge(
            store_id, &from, &to, 1.0,
            crate::store::models::ExtractionMethod::Heuristic,
        ).await?;
        count += 1;
    }

    tracing::info!("Temporal backfill complete for store {}: {} PRECEDES edges", store_id, count);
    Ok(count)
}

fn parse_ts(s: &str) -> Option<DateTime<chrono::Utc>> {
    DateTime::parse_from_rfc3339(s).ok().map(|d| d.with_timezone(&chrono::Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SurrealStore;
    use crate::store::models::ExtractionMethod;

    /// Seed two articles + one entity_overlap edge; expect one PRECEDES from
    /// the earlier to the later created_at.
    #[tokio::test]
    async fn temporal_backfill_emits_one_precedes_per_overlap_pair() {
        let store = SurrealStore::open_in_memory().await.expect("open mem");
        store.db().query(r#"
            CREATE article:earlier CONTENT { store_id: "tb1-s1", title: "E", content: "x",
                source_type: "user", source_id: "", content_hash: "tb1-e", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:later CONTENT { store_id: "tb1-s1", title: "L", content: "y",
                source_type: "user", source_id: "", content_hash: "tb1-l", tags: [],
                created_at: "2026-02-01T00:00:00Z", updated_at: "2026-02-01T00:00:00Z" };
            RELATE article:earlier->entity_overlap->article:later CONTENT {
                shared_entity_count: 2, strength: 0.5, confidence: 0.5,
                extraction_method: "heuristic", store_id: "tb1-s1",
                created_at: "2026-02-01T00:00:01Z", updated_at: "2026-02-01T00:00:01Z"
            };
        "#).await.expect("seed").check().expect("seed check");

        let n = backfill_temporal(&store, "tb1-s1").await.expect("backfill");
        assert_eq!(n, 1);

        let edges = store.list_precedes_for("tb1-s1", "earlier").await.expect("list");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].to_article_id, "later");
        assert_eq!(edges[0].extraction_method, ExtractionMethod::Heuristic);
    }

    #[tokio::test]
    async fn temporal_backfill_is_idempotent() {
        let store = SurrealStore::open_in_memory().await.expect("open mem");
        store.db().query(r#"
            CREATE article:tb2a CONTENT { store_id: "tb2-s1", title: "A", content: "",
                source_type: "user", source_id: "", content_hash: "tb2-a", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:tb2b CONTENT { store_id: "tb2-s1", title: "B", content: "",
                source_type: "user", source_id: "", content_hash: "tb2-b", tags: [],
                created_at: "2026-02-01T00:00:00Z", updated_at: "2026-02-01T00:00:00Z" };
            RELATE article:tb2a->entity_overlap->article:tb2b CONTENT {
                shared_entity_count: 1, strength: 0.3, confidence: 0.3,
                extraction_method: "heuristic", store_id: "tb2-s1",
                created_at: "2026-02-01T00:00:01Z", updated_at: "2026-02-01T00:00:01Z"
            };
        "#).await.expect("seed").check().expect("seed check");

        backfill_temporal(&store, "tb2-s1").await.expect("first");
        backfill_temporal(&store, "tb2-s1").await.expect("second");

        let edges = store.list_precedes_for("tb2-s1", "tb2a").await.expect("list");
        assert_eq!(edges.len(), 1, "duplicate not added");
    }

    #[tokio::test]
    async fn temporal_backfill_skips_equal_timestamps() {
        let store = SurrealStore::open_in_memory().await.expect("open mem");
        store.db().query(r#"
            CREATE article:tb3a CONTENT { store_id: "tb3-s1", title: "A", content: "",
                source_type: "user", source_id: "", content_hash: "tb3-a", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:tb3b CONTENT { store_id: "tb3-s1", title: "B", content: "",
                source_type: "user", source_id: "", content_hash: "tb3-b", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            RELATE article:tb3a->entity_overlap->article:tb3b CONTENT {
                shared_entity_count: 1, strength: 0.3, confidence: 0.3,
                extraction_method: "heuristic", store_id: "tb3-s1",
                created_at: "2026-01-01T00:00:01Z", updated_at: "2026-01-01T00:00:01Z"
            };
        "#).await.expect("seed").check().expect("seed check");

        let n = backfill_temporal(&store, "tb3-s1").await.expect("backfill");
        assert_eq!(n, 0, "equal timestamps yield no PRECEDES");
    }
}
