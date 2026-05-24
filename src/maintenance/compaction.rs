//! Compaction: redundant Cold-tier clusters synthesized into reflections.
//!
//! For each Cold + unpinned cluster of ≥ min_cluster_size articles sharing
//! an entity, P7 Reflector produces a single reflection Article that
//! synthesizes the cluster. Source articles are marked compacted_into =
//! reflection_id and demoted to tier = Archive.
//!
//! Quarantine semantics (Animus VectorFS principle): no source is deleted.
//! Originals remain queryable when include_archive=true; default queries
//! exclude them.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::{CompactionConfig, ExtractionConfig};
use crate::knowledge::reflection::{Reflector, ReflectionCluster};
use crate::store::{Article, Store, Tier};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CompactionReport {
    pub clusters_examined: usize,
    pub clusters_below_threshold: usize,
    pub clusters_compacted: usize,
    pub articles_archived: usize,
    /// Reflection article IDs created during this run.
    pub reflection_ids: Vec<String>,
    pub skipped_pinned: usize,
}

/// Compact Cold-tier redundant clusters in a store. Calls P7 Reflector
/// per cluster; stores the reflection; marks each source article
/// compacted_into=reflection_id + tier=Archive.
///
/// `dry_run=true` prints what would happen without writes; returns the
/// same report shape (with `reflection_ids` empty).
pub async fn compact_low_salience(
    db: Arc<dyn Store>,
    store_id: &str,
    config: &CompactionConfig,
    reflector_config: &ExtractionConfig,
    dry_run: bool,
) -> Result<CompactionReport> {
    let mut report = CompactionReport::default();

    // 1. Pull all Cold-tier articles for the store.
    let cold_articles = db.list_articles_by_tier(store_id, Tier::Cold).await?;
    if cold_articles.is_empty() {
        tracing::info!("Compaction: no Cold-tier articles in store {}; nothing to do", store_id);
        return Ok(report);
    }

    // Filter out pinned items
    let unpinned_cold: Vec<Article> = cold_articles
        .into_iter()
        .filter(|a| {
            if a.pinned {
                report.skipped_pinned += 1;
                false
            } else {
                true
            }
        })
        .collect();

    if unpinned_cold.is_empty() {
        return Ok(report);
    }

    // 2. Group articles by shared entities. Build a map: entity_id → article_ids.
    let mut clusters_by_entity: HashMap<String, Vec<String>> = HashMap::new();

    for article in &unpinned_cold {
        let entities = db.list_entities_for_article(&article.id).await
            .unwrap_or_default();
        for entity in entities {
            clusters_by_entity
                .entry(entity.id.clone())
                .or_default()
                .push(article.id.clone());
        }
    }

    // 3. For each entity-cluster ≥ min_size, attempt compaction.
    let reflector = Reflector::new(reflector_config.clone());
    let id_to_article: HashMap<String, Article> = unpinned_cold
        .iter()
        .map(|a| (a.id.clone(), a.clone()))
        .collect();

    // Sort entity-clusters by size (largest first), so we use the budget on
    // the highest-yield compactions.
    let mut entity_clusters: Vec<(String, Vec<String>)> = clusters_by_entity.into_iter().collect();
    entity_clusters.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    let mut compacted_so_far: HashSet<String> = HashSet::new();

    for (entity_id, article_ids) in entity_clusters {
        if report.clusters_compacted >= config.max_clusters_per_run {
            break;
        }
        report.clusters_examined += 1;

        // Skip articles already compacted into a prior cluster this run
        let cluster_articles: Vec<Article> = article_ids
            .iter()
            .filter(|aid| !compacted_so_far.contains(*aid))
            .filter_map(|aid| id_to_article.get(aid).cloned())
            .collect();

        if cluster_articles.len() < config.min_cluster_size {
            report.clusters_below_threshold += 1;
            continue;
        }

        let cluster = ReflectionCluster {
            sources: cluster_articles.clone(),
            intent: format!("cold-tier cluster sharing entity {}", entity_id),
        };

        if dry_run {
            println!(
                "[dry-run] Would compact {} articles sharing entity {} into a reflection",
                cluster_articles.len(),
                entity_id
            );
            report.clusters_compacted += 1;
            report.articles_archived += cluster_articles.len();
            for a in &cluster_articles {
                compacted_so_far.insert(a.id.clone());
            }
            continue;
        }

        // Run Reflector. If LLM disabled or returns None, skip.
        let reflection_result = match reflector.reflect(&cluster).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                tracing::debug!(
                    "Compaction: reflector returned None for cluster on entity {} (likely LLM disabled or empty delta)",
                    entity_id
                );
                continue;
            }
            Err(e) => {
                tracing::warn!("Compaction: reflector failed for entity {}: {}", entity_id, e);
                continue;
            }
        };

        // Store the reflection Article
        let now = chrono::Utc::now().to_rfc3339();
        let reflection_id = format!(
            "compaction-{}-{}",
            entity_id,
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );

        let reflection_article = Article {
            id: reflection_id.clone(),
            store_id: store_id.to_string(),
            title: format!("Compaction: {}", entity_id),
            content: reflection_result.delta_summary,
            source_type: "reflection".into(),
            source_id: String::new(),
            content_hash: format!("compaction-{}", reflection_id),
            tags: serde_json::json!([]),
            embedded_at: None,
            created_at: now.clone(),
            updated_at: now,
            reflects: reflection_result.source_ids.clone(),
            access_count: 0,
            last_accessed_at: String::new(),
            importance_score: reflection_result.raw_confidence,
            tier: Tier::Hot,
            pinned: false,
            compacted_into: None,
        };

        db.create_article(&reflection_article).await
            .with_context(|| format!("Failed to store compaction reflection {}", reflection_id))?;

        // Mark each source as compacted (sets tier=Archive + audit log entry)
        for source_id in &reflection_result.source_ids {
            db.set_article_compacted_into(source_id, &reflection_id).await?;
            compacted_so_far.insert(source_id.clone());
            report.articles_archived += 1;
        }

        report.clusters_compacted += 1;
        report.reflection_ids.push(reflection_id);
    }

    tracing::info!(
        "Compaction for store {} complete: {} clusters compacted, {} articles archived, {} skipped",
        store_id,
        report.clusters_compacted,
        report.articles_archived,
        report.skipped_pinned
    );

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CompactionConfig, ExtractionConfig};
    use crate::store::{Article, Entity, SurrealStore, Tier};

    fn make_cold_article(id: &str, pinned: bool) -> Article {
        Article {
            id: id.into(),
            store_id: "comp-s1".into(),
            title: format!("Cold {}", id),
            content: format!("cold content of {}", id),
            source_type: "user".into(),
            source_id: String::new(),
            content_hash: format!("{}-h", id),
            tags: serde_json::json!([]),
            embedded_at: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            reflects: vec![],
            access_count: 0,
            last_accessed_at: "2026-01-01T00:00:00Z".into(),
            importance_score: 0.1,
            tier: Tier::Cold,
            pinned,
            compacted_into: None,
        }
    }

    async fn seed_cluster_fixture() -> Arc<dyn Store> {
        let store = SurrealStore::open_in_memory().await.unwrap();

        // 6 Cold-tier articles, one pinned
        for (id, pinned) in &[
            ("comp-a1", false),
            ("comp-a2", false),
            ("comp-a3", false),
            ("comp-a4", false),
            ("comp-a5", false),
            ("comp-pin", true),  // pinned: should be excluded
        ] {
            store.create_article(&make_cold_article(id, *pinned)).await.unwrap();
        }

        // Shared entity "outage"
        store.create_entity(&Entity {
            id: "comp-ent-outage".into(),
            name: "outage".into(),
            entity_type: "concept".into(),
            description: None,
            store_id: "comp-s1".into(),
            mention_count: 6,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        }).await.unwrap();

        // All 6 articles mention "outage"
        for aid in &["comp-a1", "comp-a2", "comp-a3", "comp-a4", "comp-a5", "comp-pin"] {
            store.create_mentions_edge(aid, "comp-ent-outage", "outage", 0.9).await.unwrap();
        }

        Arc::new(store)
    }

    #[tokio::test]
    async fn dry_run_does_not_modify_store() {
        let db = seed_cluster_fixture().await;
        let cfg = CompactionConfig::default();
        let llm_cfg = ExtractionConfig { enabled: false, ..Default::default() };

        let report = compact_low_salience(db.clone(), "comp-s1", &cfg, &llm_cfg, true)
            .await.unwrap();

        // 5 unpinned articles form a single cluster of size 5 (>= min_cluster_size)
        assert_eq!(report.clusters_examined, 1);
        assert_eq!(report.clusters_compacted, 1);
        assert_eq!(report.articles_archived, 5);
        assert!(report.reflection_ids.is_empty(), "dry-run must not create reflections");

        // Verify no Article was modified
        let a1 = db.get_article("comp-a1").await.unwrap().unwrap();
        assert_eq!(a1.tier, Tier::Cold, "dry-run must not change tier");
        assert!(a1.compacted_into.is_none(), "dry-run must not set compacted_into");
    }

    #[tokio::test]
    async fn pinned_articles_excluded_from_compaction() {
        let db = seed_cluster_fixture().await;
        let cfg = CompactionConfig::default();
        let llm_cfg = ExtractionConfig { enabled: false, ..Default::default() };

        let report = compact_low_salience(db.clone(), "comp-s1", &cfg, &llm_cfg, true)
            .await.unwrap();

        assert_eq!(report.skipped_pinned, 1);

        // Pinned article must still be there in Cold tier
        let pinned = db.get_article("comp-pin").await.unwrap().unwrap();
        assert!(pinned.pinned);
        assert_eq!(pinned.tier, Tier::Cold);
        assert!(pinned.compacted_into.is_none());
    }

    #[tokio::test]
    async fn below_min_cluster_size_skipped() {
        let s = SurrealStore::open_in_memory().await.unwrap();

        // Seed a single Cold article (cluster size = 1, below min=5)
        s.create_article(&make_cold_article("comp-lonely", false)).await.unwrap();
        s.create_entity(&Entity {
            id: "comp-lonely-ent".into(),
            name: "lonely".into(),
            entity_type: "concept".into(),
            description: None,
            store_id: "comp-s1".into(),
            mention_count: 1,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        }).await.unwrap();
        s.create_mentions_edge("comp-lonely", "comp-lonely-ent", "lonely", 0.9).await.unwrap();

        let db_solo: Arc<dyn Store> = Arc::new(s);
        let cfg = CompactionConfig::default();
        let llm_cfg = ExtractionConfig { enabled: false, ..Default::default() };

        let report = compact_low_salience(db_solo, "comp-s1", &cfg, &llm_cfg, true)
            .await.unwrap();

        assert_eq!(report.clusters_compacted, 0);
        assert!(report.clusters_below_threshold >= 1 || report.clusters_examined == 0);
    }
}
