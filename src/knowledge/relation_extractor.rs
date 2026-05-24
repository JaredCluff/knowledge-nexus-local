//! Orchestrates the four P5 backfill paths: temporal, semantic, citations,
//! causal. Each is independently toggleable; counts are returned per method.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::config::GraphConfig;
use crate::knowledge::{citation_backfill, semantic_backfill, temporal_backfill};
use crate::store::Store;
use crate::vectordb::VectorDbBackfillApi;

/// Which paths to run. All independent; any combination is valid.
#[derive(Debug, Clone, Default)]
pub struct ExtractRelationsRequest {
    pub temporal: bool,
    pub semantic: bool,
    pub citations: bool,
    pub causal: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtractRelationsReport {
    pub temporal_edges: u64,
    pub semantic_edges: u64,
    pub citation_edges: u64,
    pub causal_edges: u64,
}

pub async fn extract_relations<S, V>(
    store: &S,
    vector_db: &V,
    config: &GraphConfig,
    store_id: &str,
    req: ExtractRelationsRequest,
) -> Result<ExtractRelationsReport>
where
    S: Store + Sync + ?Sized,
    V: VectorDbBackfillApi + Sync + ?Sized,
{
    let mut report = ExtractRelationsReport {
        temporal_edges: 0,
        semantic_edges: 0,
        citation_edges: 0,
        causal_edges: 0,
    };

    if req.temporal {
        report.temporal_edges = temporal_backfill::backfill_temporal(store, store_id).await?;
    }
    if req.semantic {
        report.semantic_edges = semantic_backfill::backfill_semantic(
            store, vector_db, store_id,
            config.semantic_threshold, config.semantic_top_k,
        ).await?;
    }
    if req.citations {
        report.citation_edges = citation_backfill::backfill_citations(store, store_id).await?;
    }
    if req.causal {
        // Causal backfill is a slow LLM path; bounded by the entity-overlap
        // graph (avoids O(N²) LLM calls).
        report.causal_edges = run_causal_backfill(store, config, store_id).await?;
    }
    Ok(report)
}

async fn run_causal_backfill<S: Store + Sync + ?Sized>(
    store: &S,
    config: &GraphConfig,
    store_id: &str,
) -> Result<u64> {
    use crate::knowledge::causal_extractor::CausalExtractor;
    let extractor = CausalExtractor::new(config.clone(), config.ollama_url.clone());

    let pairs = store.list_entity_overlap_pairs(store_id).await?;
    let mut count = 0u64;

    for (a_id, b_id) in pairs {
        let Some(a) = store.get_article(&a_id).await? else { continue };
        let Some(b) = store.get_article(&b_id).await? else { continue };

        let excerpt_a = truncate(&a.content, 600);
        let excerpt_b = truncate(&b.content, 600);

        if let Ok(Some(claim)) = extractor.extract(&a.title, excerpt_a, &b.title, excerpt_b).await {
            if claim.confidence >= config.causal_confidence_threshold {
                store.create_caused_by_edge(store_id, &a_id, &b_id, claim.confidence, claim.rationale).await?;
                count += 1;
            }
        }
    }

    Ok(count)
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        // Find the largest char boundary <= max
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SurrealStore;
    use crate::vectordb::mock::MockVectorDb;

    #[tokio::test]
    async fn extract_relations_runs_only_requested_methods() {
        let store = SurrealStore::open_in_memory().await.expect("open mem");
        store.db().query(r#"
            CREATE article:reorch_a CONTENT { store_id: "re1-s1", title: "A", content: "",
                source_type: "user", source_id: "", content_hash: "re1-a", tags: [],
                created_at: "2026-01-01T00:00:00Z", updated_at: "2026-01-01T00:00:00Z" };
            CREATE article:reorch_b CONTENT { store_id: "re1-s1", title: "B", content: "",
                source_type: "user", source_id: "", content_hash: "re1-b", tags: [],
                created_at: "2026-02-01T00:00:00Z", updated_at: "2026-02-01T00:00:00Z" };
            RELATE article:reorch_a->entity_overlap->article:reorch_b CONTENT {
                shared_entity_count: 1, strength: 0.3, confidence: 0.3,
                extraction_method: "heuristic", store_id: "re1-s1",
                created_at: "2026-02-01T00:00:01Z", updated_at: "2026-02-01T00:00:01Z"
            };
        "#).await.expect("seed").check().expect("seed check");

        let cfg = GraphConfig::default(); // causal_enabled = false
        let mock = MockVectorDb::with_pairs("re1-s1", &[]);
        let req = ExtractRelationsRequest { temporal: true, semantic: false, citations: false, causal: false };

        let report = extract_relations(&store, &mock, &cfg, "re1-s1", req).await.expect("extract");

        assert_eq!(report.temporal_edges, 1);
        assert_eq!(report.semantic_edges, 0);
        assert_eq!(report.citation_edges, 0);
        assert_eq!(report.causal_edges, 0);
    }

    /// E2E backfill test on a 10-article fixture. Exercises temporal + semantic
    /// + citation paths together (causal is opt-in and requires Ollama; skipped).
    #[tokio::test]
    async fn e2e_backfill_emits_expected_edges_on_10_article_fixture() {
        let store = crate::store::SurrealStore::open_in_memory().await.expect("open mem");

        // Seed 10 articles with deterministic timestamps in 2026-01-01..2026-01-10
        let mut seed_q = String::new();
        for i in 1..=10usize {
            let day = i;
            // Article 5 cites article 3 via markdown link
            let content = if i == 5 {
                "see [retro](e2ea3) for prior context".to_string()
            } else {
                String::new()
            };
            seed_q.push_str(&format!(
                "CREATE article:e2ea{i} CONTENT {{ store_id: \"e2e-s1\", title: \"A{i}\",
                    content: \"{content}\",
                    source_type: \"user\", source_id: \"\", content_hash: \"e2ea{i}\",
                    tags: [], created_at: \"2026-01-{day:02}T00:00:00Z\",
                    updated_at: \"2026-01-{day:02}T00:00:00Z\" }};\n",
            ));
        }
        // 3 entity-overlap pairs: (a1,a2), (a3,a5), (a7,a8) — seeded directly into entity_overlap
        for (a, b) in &[(1usize, 2usize), (3, 5), (7, 8)] {
            seed_q.push_str(&format!(
                "RELATE article:e2ea{a}->entity_overlap->article:e2ea{b} CONTENT {{
                    shared_entity_count: 2, strength: 0.5, confidence: 0.5,
                    extraction_method: \"heuristic\", store_id: \"e2e-s1\",
                    created_at: \"2026-01-10T00:00:00Z\", updated_at: \"2026-01-10T00:00:00Z\"
                }};\n",
            ));
        }
        store.db().query(&seed_q).await.expect("seed").check().expect("seed check");

        // MockVectorDb: e2ea2 <-> e2ea3 are very similar (0.95); other neighbors <0.6
        let mock = MockVectorDb::with_pairs("e2e-s1", &[
            ("e2ea1",  &[("e2ea2", 0.5), ("e2ea3", 0.5)][..]),
            ("e2ea2",  &[("e2ea3", 0.95), ("e2ea1", 0.5)][..]),
            ("e2ea3",  &[("e2ea2", 0.95), ("e2ea5", 0.5)][..]),
            ("e2ea4",  &[("e2ea5", 0.5)][..]),
            ("e2ea5",  &[("e2ea3", 0.5)][..]),
            ("e2ea6",  &[("e2ea7", 0.5)][..]),
            ("e2ea7",  &[("e2ea8", 0.5), ("e2ea6", 0.5)][..]),
            ("e2ea8",  &[("e2ea7", 0.5)][..]),
            ("e2ea9",  &[("e2ea10", 0.5)][..]),
            ("e2ea10", &[("e2ea9", 0.5)][..]),
        ]);

        let cfg = crate::config::GraphConfig {
            semantic_threshold: 0.85,
            semantic_top_k: 5,
            ..Default::default()
        };
        let req = ExtractRelationsRequest {
            temporal: true,
            semantic: true,
            citations: true,
            causal: false,
        };
        let report = extract_relations(&store, &mock, &cfg, "e2e-s1", req).await.expect("extract");

        // Temporal: 3 PRECEDES edges, one per entity_overlap pair (all pairs have
        // distinct created_at: e.g. day 1 < day 2)
        assert_eq!(report.temporal_edges, 3, "expected 3 temporal edges from 3 overlap pairs");

        // Semantic: e2ea2-e2ea3 pair. Two calls attempted (a2->a3, a3->a2 ordered
        // lex by from < to so both write the same edge). UNIQUE index dedups to 1.
        // The returned count is the number of CALLS, which is 2.
        assert!(report.semantic_edges >= 1, "at least one semantic edge create attempted");

        // Citations: a5 -> a3 (one explicit markdown link)
        assert_eq!(report.citation_edges, 1);

        // Causal disabled.
        assert_eq!(report.causal_edges, 0);

        // Verify per-edge-type counts via Store::count_edges_by_type
        let counts = store.count_edges_by_type("e2e-s1").await.expect("counts");
        assert_eq!(counts.entity_overlap, 3, "seeded entity_overlap edges unchanged");
        assert_eq!(counts.precedes, 3);
        assert_eq!(counts.semantically_related, 1, "one unique semantic edge after dedup");
        assert_eq!(counts.caused_by, 0);
        assert_eq!(counts.references_edge, 1);
    }
}
