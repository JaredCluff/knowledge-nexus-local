//! Semantic-edge backfill via LanceDB ANN.
//!
//! For each article with a stored embedding, query LanceDB for nearest
//! neighbors; emit a SEMANTICALLY_RELATED edge to each neighbor whose
//! cosine similarity exceeds a configurable threshold (default 0.85).
//! Directional dedup: only emit (from, to) where from_id < to_id, since
//! the UNIQUE index would reject both directions anyway.

use anyhow::Result;

use crate::store::Store;
use crate::vectordb::VectorDbBackfillApi;

/// Per-store semantic backfill. Returns the number of SEMANTICALLY_RELATED
/// edge create-calls attempted (calls may be no-ops if a previous run
/// already created the edge).
pub async fn backfill_semantic<S: Store + Sync + ?Sized>(
    store: &S,
    vector_db: &(impl VectorDbBackfillApi + ?Sized),
    store_id: &str,
    threshold: f64,
    top_k: usize,
) -> Result<u64> {
    let article_ids = store.list_article_ids(store_id).await?;
    let mut count = 0u64;

    for article_id in &article_ids {
        let Some(emb) = vector_db.get_embedding(store_id, article_id).await? else { continue };
        let neighbors = vector_db.ann_query(store_id, &emb, top_k).await?;

        for (neighbor_id, similarity) in neighbors {
            if neighbor_id == *article_id { continue; }
            if similarity < threshold { continue; }
            // Lexicographic ordering ensures we don't create both A->B and B->A.
            let (from, to) = if article_id.as_str() < neighbor_id.as_str() {
                (article_id.clone(), neighbor_id.clone())
            } else {
                (neighbor_id.clone(), article_id.clone())
            };
            store.create_semantically_related_edge(store_id, &from, &to, similarity).await?;
            count += 1;
        }
    }

    tracing::info!(
        "Semantic backfill complete for store {}: {} edges (threshold={}, top_k={})",
        store_id, count, threshold, top_k
    );
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SurrealStore;
    use crate::vectordb::mock::MockVectorDb;

    #[tokio::test]
    async fn semantic_backfill_emits_edges_above_threshold() {
        let store = SurrealStore::open_in_memory().await.expect("open mem");
        store.db().query(r#"
            CREATE article:sba CONTENT { store_id: "sb1-s1", title: "A", content: "",
                source_type: "user", source_id: "", content_hash: "sb1-a", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:sbb CONTENT { store_id: "sb1-s1", title: "B", content: "",
                source_type: "user", source_id: "", content_hash: "sb1-b", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:sbc CONTENT { store_id: "sb1-s1", title: "C", content: "",
                source_type: "user", source_id: "", content_hash: "sb1-c", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
        "#).await.expect("seed").check().expect("seed check");

        // sba-sbb are similar (0.92); sba-sbc are not (0.6).
        let mock = MockVectorDb::with_pairs("sb1-s1", &[
            ("sba", &[("sbb", 0.92), ("sbc", 0.6)]),
            ("sbb", &[("sba", 0.92), ("sbc", 0.6)]),
            ("sbc", &[("sba", 0.6), ("sbb", 0.6)]),
        ]);

        let _ = backfill_semantic(&store, &mock, "sb1-s1", 0.85, 10).await.expect("backfill");

        // Both sba->sbb iterations attempt RELATE sba->semantically_related->sbb
        // (lex-ordered). The unique index swallows the duplicate. Net: one edge.
        let edges = store.list_semantically_related_for("sb1-s1", "sba").await.expect("list a");
        assert_eq!(edges.len(), 1, "exactly one semantic edge surfaces under sba");
        assert!((edges[0].similarity - 0.92).abs() < 1e-9);
    }

    #[tokio::test]
    async fn semantic_backfill_respects_threshold() {
        let store = SurrealStore::open_in_memory().await.expect("open mem");
        store.db().query(r#"
            CREATE article:sbra CONTENT { store_id: "sb2-s1", title: "A", content: "",
                source_type: "user", source_id: "", content_hash: "sb2-a", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:sbrb CONTENT { store_id: "sb2-s1", title: "B", content: "",
                source_type: "user", source_id: "", content_hash: "sb2-b", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
        "#).await.expect("seed").check().expect("seed check");

        // Both pairs are below threshold (0.7 < 0.85).
        let mock = MockVectorDb::with_pairs("sb2-s1", &[
            ("sbra", &[("sbrb", 0.7)]),
            ("sbrb", &[("sbra", 0.7)]),
        ]);
        let _ = backfill_semantic(&store, &mock, "sb2-s1", 0.85, 10).await.expect("backfill");
        let edges = store.list_semantically_related_for("sb2-s1", "sbra").await.expect("list");
        assert_eq!(edges.len(), 0, "below-threshold neighbors must be skipped");
    }
}
