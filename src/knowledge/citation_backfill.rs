//! Citation-edge backfill via markdown link parsing.
//!
//! Looks for `[anchor](article_id)` patterns in article content where
//! `article_id` matches an existing article in the same store. Emits a
//! REFERENCES_EDGE for each match. Cheap; idempotent; user-asserted.

use anyhow::Result;
use regex::Regex;

use crate::store::Store;

/// Per-store citation backfill. Returns the number of REFERENCES_EDGE
/// create-calls attempted (calls may be no-ops if a previous run already
/// created the edge via the UNIQUE index).
pub async fn backfill_citations<S: Store + Sync + ?Sized>(store: &S, store_id: &str) -> Result<u64> {
    let re = Regex::new(r"\[([^\]]+)\]\(([a-zA-Z0-9_-]+)\)").expect("static regex compiles");

    let ids = store.list_article_ids(store_id).await?;
    let id_set: std::collections::HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
    let mut count = 0u64;

    for from_id in &ids {
        let Some(article) = store.get_article(from_id).await? else { continue };
        for cap in re.captures_iter(&article.content) {
            let anchor = cap.get(1).map(|m| m.as_str().to_string());
            let target = cap.get(2).map(|m| m.as_str().to_string());
            let Some(target) = target else { continue };
            if target == *from_id { continue; }
            if !id_set.contains(target.as_str()) { continue; }

            store.create_references_edge(store_id, from_id, &target, anchor).await?;
            count += 1;
        }
    }

    tracing::info!("Citation backfill complete for store {}: {} REFERENCES edges", store_id, count);
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SurrealStore;

    #[tokio::test]
    async fn citation_backfill_emits_edge_for_existing_target() {
        let store = SurrealStore::open_in_memory().await.expect("open mem");
        store.db().query(r#"
            CREATE article:cbsrc CONTENT { store_id: "cb1-s1", title: "S",
                content: "see [the retro](cbtgt) for context",
                source_type: "user", source_id: "", content_hash: "cb1-s", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:cbtgt CONTENT { store_id: "cb1-s1", title: "T", content: "",
                source_type: "user", source_id: "", content_hash: "cb1-t", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
        "#).await.expect("seed").check().expect("seed check");

        let n = backfill_citations(&store, "cb1-s1").await.expect("backfill");
        assert_eq!(n, 1);

        let edges = store.list_references_for("cb1-s1", "cbsrc").await.expect("list");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].anchor_text.as_deref(), Some("the retro"));
    }

    #[tokio::test]
    async fn citation_backfill_skips_unknown_targets() {
        let store = SurrealStore::open_in_memory().await.expect("open mem");
        store.db().query(r#"
            CREATE article:cb2src CONTENT { store_id: "cb2-s1", title: "S",
                content: "see [missing](does_not_exist)",
                source_type: "user", source_id: "", content_hash: "cb2-s", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
        "#).await.expect("seed").check().expect("seed check");

        let n = backfill_citations(&store, "cb2-s1").await.expect("backfill");
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn citation_backfill_skips_self_references() {
        let store = SurrealStore::open_in_memory().await.expect("open mem");
        store.db().query(r#"
            CREATE article:cb3src CONTENT { store_id: "cb3-s1", title: "S",
                content: "see [self](cb3src)",
                source_type: "user", source_id: "", content_hash: "cb3-s", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
        "#).await.expect("seed").check().expect("seed check");

        let n = backfill_citations(&store, "cb3-s1").await.expect("backfill");
        assert_eq!(n, 0);
    }
}
