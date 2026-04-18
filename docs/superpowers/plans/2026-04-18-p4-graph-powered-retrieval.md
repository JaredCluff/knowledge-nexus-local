# P4: Graph-Powered Tri-Signal Retrieval — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Integrate the P3 knowledge graph into the search pipeline as a third retrieval signal alongside vector similarity and full-text search, unify the CLI onto the hybrid pipeline, and add a graph debug command.

**Architecture:** A new `GraphSearcher` component matches query terms against entities, traverses MENTIONS/RELATED_TO edges to find connected articles, and produces a ranked list. The existing two-way RRF merge is generalized to N-way with adaptive graph weighting based on entity coverage. The CLI `search` command is rewired to use `LocalRouter::route()` instead of `search::search_files()`.

**Tech Stack:** Rust, SurrealDB (graph traversals), existing retrieval pipeline (RRF, reranking, expansion), clap (CLI)

---

### Task 1: Add `RetrievalConfig` to configuration

**Files:**
- Modify: `src/config/mod.rs`

- [ ] **Step 1: Add the `RetrievalConfig` struct and defaults**

In `src/config/mod.rs`, after the `ExtractionConfig` section (after line 278), add:

```rust
/// Retrieval pipeline configuration (P4)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalConfig {
    /// RRF constant (higher = more weight to top ranks)
    #[serde(default = "default_rrf_k")]
    pub rrf_k: f32,

    /// Weight for vector search signal in RRF
    #[serde(default = "default_vector_weight")]
    pub vector_weight: f32,

    /// Weight for keyword/FTS search signal in RRF
    #[serde(default = "default_keyword_weight")]
    pub keyword_weight: f32,

    /// Maximum weight for graph search signal (scaled by entity coverage)
    #[serde(default = "default_graph_weight_max")]
    pub graph_weight_max: f32,

    /// Max hops for RELATED_TO traversal (1 = direct neighbors only)
    #[serde(default = "default_graph_hops")]
    pub graph_hops: usize,
}

fn default_rrf_k() -> f32 { 60.0 }
fn default_vector_weight() -> f32 { 1.0 }
fn default_keyword_weight() -> f32 { 1.1 }
fn default_graph_weight_max() -> f32 { 1.0 }
fn default_graph_hops() -> usize { 1 }

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            rrf_k: default_rrf_k(),
            vector_weight: default_vector_weight(),
            keyword_weight: default_keyword_weight(),
            graph_weight_max: default_graph_weight_max(),
            graph_hops: default_graph_hops(),
        }
    }
}
```

- [ ] **Step 2: Add `retrieval` field to `Config` struct**

In the `Config` struct (around line 17), add after the `extraction` field:

```rust
    /// Retrieval pipeline settings (P4)
    #[serde(default)]
    pub retrieval: RetrievalConfig,
```

- [ ] **Step 3: Run tests to verify config deserialization**

Run: `cargo test --lib -- config 2>&1 | tail -5`
Expected: All existing config tests pass. A missing `[retrieval]` section defaults gracefully.

- [ ] **Step 4: Commit**

```bash
git add src/config/mod.rs
git commit -m "feat(config): add RetrievalConfig for tri-signal search tuning"
```

---

### Task 2: Add graph query methods to Store trait

**Files:**
- Modify: `src/store/mod.rs`

- [ ] **Step 1: Write failing tests for new graph query methods**

In `src/store/mod.rs`, inside the `#[cfg(test)] mod entity_tests` block (at the end, before the closing `}`), add:

```rust
    #[tokio::test]
    async fn test_search_entities_by_name() {
        let s = fixture().await;
        let ts = now();

        // Create entities
        s.create_entity(&Entity {
            id: "tool:rust".into(), name: "Rust".into(), entity_type: "tool".into(),
            description: Some("Systems language".into()), store_id: "s1".into(),
            mention_count: 5, created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_entity(&Entity {
            id: "tool:tokio".into(), name: "Tokio".into(), entity_type: "tool".into(),
            description: Some("Async runtime".into()), store_id: "s1".into(),
            mention_count: 3, created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_entity(&Entity {
            id: "concept:async-runtime".into(), name: "async runtime".into(),
            entity_type: "concept".into(), description: None, store_id: "s1".into(),
            mention_count: 2, created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        // Exact match
        let results = s.search_entities_by_name("s1", &["Rust"]).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "tool:rust");

        // Prefix match
        let results = s.search_entities_by_name("s1", &["Tok"]).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "tool:tokio");

        // Multi-word match
        let results = s.search_entities_by_name("s1", &["async"]).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "concept:async-runtime");

        // No match
        let results = s.search_entities_by_name("s1", &["Python"]).await.unwrap();
        assert!(results.is_empty());

        // Multiple terms
        let results = s.search_entities_by_name("s1", &["Rust", "Tokio"]).await.unwrap();
        assert_eq!(results.len(), 2);

        // Wrong store
        let results = s.search_entities_by_name("s2", &["Rust"]).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_list_articles_for_entities() {
        let s = fixture().await;
        let ts = now();

        // Create articles
        s.create_article(&Article {
            id: "a1".into(), store_id: "s1".into(), title: "Rust Guide".into(),
            content: "About Rust".into(), source_type: "user".into(),
            source_id: String::new(), content_hash: "h1".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_article(&Article {
            id: "a2".into(), store_id: "s1".into(), title: "Tokio Deep Dive".into(),
            content: "About Tokio".into(), source_type: "user".into(),
            source_id: String::new(), content_hash: "h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        // Create entities
        s.create_entity(&Entity {
            id: "tool:rust".into(), name: "Rust".into(), entity_type: "tool".into(),
            description: None, store_id: "s1".into(), mention_count: 1,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        // Create mentions edges
        s.create_mentions_edge("a1", "tool:rust", "written in Rust", 0.95).await.unwrap();
        s.create_mentions_edge("a2", "tool:rust", "uses Rust", 0.80).await.unwrap();

        // Batch lookup
        let results = s.list_articles_for_entities(&["tool:rust"]).await.unwrap();
        assert_eq!(results.len(), 2);
        // Should include confidence
        assert!(results.iter().any(|(a, c)| a.id == "a1" && (*c - 0.95).abs() < 0.01));
        assert!(results.iter().any(|(a, c)| a.id == "a2" && (*c - 0.80).abs() < 0.01));

        // Empty input
        let results = s.list_articles_for_entities(&[]).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_count_entities_by_type() {
        let s = fixture().await;
        let ts = now();

        s.create_entity(&Entity {
            id: "tool:rust".into(), name: "Rust".into(), entity_type: "tool".into(),
            description: None, store_id: "s1".into(), mention_count: 1,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_entity(&Entity {
            id: "tool:tokio".into(), name: "Tokio".into(), entity_type: "tool".into(),
            description: None, store_id: "s1".into(), mention_count: 1,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_entity(&Entity {
            id: "person:linus".into(), name: "Linus Torvalds".into(), entity_type: "person".into(),
            description: None, store_id: "s1".into(), mention_count: 1,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        let counts = s.count_entities_by_type("s1").await.unwrap();
        assert_eq!(counts.get("tool"), Some(&2));
        assert_eq!(counts.get("person"), Some(&1));
        assert_eq!(counts.get("concept"), None);
    }

    #[tokio::test]
    async fn test_list_co_mentioned_entities() {
        let s = fixture().await;
        let ts = now();

        // Create articles
        s.create_article(&Article {
            id: "a1".into(), store_id: "s1".into(), title: "Rust Async".into(),
            content: "C".into(), source_type: "user".into(),
            source_id: String::new(), content_hash: "h1".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_article(&Article {
            id: "a2".into(), store_id: "s1".into(), title: "More Rust".into(),
            content: "C".into(), source_type: "user".into(),
            source_id: String::new(), content_hash: "h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        // Create entities
        for (id, name, etype) in [
            ("tool:rust", "Rust", "tool"),
            ("tool:tokio", "Tokio", "tool"),
            ("concept:async", "async", "concept"),
        ] {
            s.create_entity(&Entity {
                id: id.into(), name: name.into(), entity_type: etype.into(),
                description: None, store_id: "s1".into(), mention_count: 1,
                created_at: ts.clone(), updated_at: ts.clone(),
            }).await.unwrap();
        }

        // a1 mentions rust + tokio, a2 mentions rust + async
        s.create_mentions_edge("a1", "tool:rust", "e", 0.9).await.unwrap();
        s.create_mentions_edge("a1", "tool:tokio", "e", 0.9).await.unwrap();
        s.create_mentions_edge("a2", "tool:rust", "e", 0.9).await.unwrap();
        s.create_mentions_edge("a2", "concept:async", "e", 0.9).await.unwrap();

        // Co-mentioned with rust: tokio (1 shared article a1) + async (1 shared article a2)
        let co = s.list_co_mentioned_entities("tool:rust").await.unwrap();
        assert_eq!(co.len(), 2);
        // Both have 1 shared article
        assert!(co.iter().all(|(_, count)| *count == 1));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib -- entity_tests::test_search_entities_by_name 2>&1 | tail -5`
Expected: FAIL — `search_entities_by_name` method does not exist on Store trait.

- [ ] **Step 3: Add new methods to the Store trait**

In `src/store/mod.rs`, in the `Store` trait (after `list_articles_without_mentions`, around line 130), add:

```rust
    // Graph queries (P4)
    async fn search_entities_by_name(&self, store_id: &str, terms: &[&str]) -> Result<Vec<Entity>>;
    async fn list_articles_for_entities(&self, entity_ids: &[&str]) -> Result<Vec<(Article, f64)>>;
    async fn count_entities_by_type(&self, store_id: &str) -> Result<std::collections::HashMap<String, usize>>;
    async fn list_co_mentioned_entities(&self, entity_id: &str) -> Result<Vec<(Entity, usize)>>;
```

- [ ] **Step 4: Implement `search_entities_by_name` on SurrealStore**

In `src/store/mod.rs`, in the `impl Store for SurrealStore` block (after `list_articles_without_mentions` impl), add:

```rust
    async fn search_entities_by_name(&self, store_id: &str, terms: &[&str]) -> Result<Vec<Entity>> {
        if terms.is_empty() {
            return Ok(vec![]);
        }
        // Build OR conditions for exact and prefix matches (case-insensitive)
        let mut conditions = Vec::new();
        let mut binds: Vec<(String, String)> = Vec::new();
        for (i, term) in terms.iter().enumerate() {
            let lower = term.to_lowercase();
            let param = format!("term_{}", i);
            conditions.push(format!(
                "(string::lowercase(name) = ${p} OR string::lowercase(name) CONTAINS ${p})",
                p = param
            ));
            binds.push((param, lower));
        }
        let where_clause = conditions.join(" OR ");
        let query = format!(
            "SELECT * FROM entity WHERE store_id = $store_id AND ({}) ORDER BY mention_count DESC",
            where_clause
        );
        let mut q = self.db().query(&query).bind(("store_id", store_id.to_string()));
        for (param, value) in binds {
            q = q.bind((param, value));
        }
        let mut resp = q.await.context("search_entities_by_name query failed")?;
        let rows: Vec<EntityRow> = resp.take(0).unwrap_or_default();
        Ok(rows.into_iter().map(|r| r.into_entity()).collect())
    }
```

- [ ] **Step 5: Implement `list_articles_for_entities` on SurrealStore**

```rust
    async fn list_articles_for_entities(&self, entity_ids: &[&str]) -> Result<Vec<(Article, f64)>> {
        if entity_ids.is_empty() {
            return Ok(vec![]);
        }
        // Query MENTIONS edges for all given entities, join with article
        let ids: Vec<String> = entity_ids.iter().map(|id| format!("entity:{}", id)).collect();
        let mut resp = self.db()
            .query(
                "SELECT
                    meta::id(in) AS article_id,
                    confidence
                 FROM mentions
                 WHERE out IN $entity_ids"
            )
            .bind(("entity_ids", ids))
            .await
            .context("list_articles_for_entities query failed")?;
        let edges: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();

        // Collect unique article IDs with max confidence
        let mut article_confidences: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        for edge in &edges {
            let aid = edge.get("article_id").and_then(|v| v.as_str()).unwrap_or_default();
            let conf = edge.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let entry = article_confidences.entry(aid.to_string()).or_insert(0.0);
            if conf > *entry {
                *entry = conf;
            }
        }

        // Fetch articles
        let mut results = Vec::new();
        for (aid, confidence) in &article_confidences {
            if let Some(article) = self.get_article(aid).await? {
                results.push((article, *confidence));
            }
        }
        // Sort by confidence descending
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(results)
    }
```

- [ ] **Step 6: Implement `count_entities_by_type` on SurrealStore**

```rust
    async fn count_entities_by_type(&self, store_id: &str) -> Result<std::collections::HashMap<String, usize>> {
        let mut resp = self.db()
            .query("SELECT entity_type, count() AS count FROM entity WHERE store_id = $store_id GROUP BY entity_type")
            .bind(("store_id", store_id.to_string()))
            .await
            .context("count_entities_by_type query failed")?;
        let rows: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();

        let mut counts = std::collections::HashMap::new();
        for row in rows {
            let etype = row.get("entity_type").and_then(|v| v.as_str()).unwrap_or_default().to_string();
            let count = row.get("count").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            counts.insert(etype, count);
        }
        Ok(counts)
    }
```

- [ ] **Step 7: Implement `list_co_mentioned_entities` on SurrealStore**

```rust
    async fn list_co_mentioned_entities(&self, entity_id: &str) -> Result<Vec<(Entity, usize)>> {
        // Find articles mentioning this entity, then find other entities those articles mention
        let entity_thing = format!("entity:{}", entity_id);
        let mut resp = self.db()
            .query(
                "LET $articles = (SELECT VALUE in FROM mentions WHERE out = $entity_id);
                 SELECT
                    meta::id(out) AS co_entity_id,
                    count() AS shared_count
                 FROM mentions
                 WHERE in IN $articles AND out != $entity_id
                 GROUP BY out
                 ORDER BY shared_count DESC"
            )
            .bind(("entity_id", entity_thing))
            .await
            .context("list_co_mentioned_entities query failed")?;
        // Statement 0 is the LET, statement 1 is the SELECT
        let rows: Vec<serde_json::Value> = resp.take(1).unwrap_or_default();

        let mut results = Vec::new();
        for row in rows {
            let co_id = row.get("co_entity_id").and_then(|v| v.as_str()).unwrap_or_default();
            let count = row.get("shared_count").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            if let Some(entity) = self.get_entity(co_id).await? {
                results.push((entity, count));
            }
        }
        Ok(results)
    }
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test --lib -- entity_tests::test_search_entities_by_name entity_tests::test_list_articles_for_entities entity_tests::test_count_entities_by_type entity_tests::test_list_co_mentioned_entities 2>&1 | tail -10`
Expected: All 4 new tests pass.

- [ ] **Step 9: Commit**

```bash
git add src/store/mod.rs
git commit -m "feat(store): add P4 graph query methods — entity search, batch article lookup, co-mentions"
```

---

### Task 3: Implement `GraphSearcher`

**Files:**
- Create: `src/retrieval/graph.rs`
- Modify: `src/retrieval/mod.rs`

- [ ] **Step 1: Create `src/retrieval/graph.rs` with struct and constructor**

```rust
//! Graph-based retrieval: matches query terms to entities, traverses
//! MENTIONS and RELATED_TO edges, and produces a ranked result list.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use tracing::debug;

use crate::config::RetrievalConfig;
use crate::k2k::models::{K2KResult, ResultProvenance};
use crate::retrieval::expansion::QueryExpander;
use crate::store::Store;

/// Result of a graph search pass, including entity coverage metadata
/// needed for adaptive RRF weighting.
pub struct GraphSearchOutput {
    /// Ranked results from graph traversal.
    pub results: Vec<K2KResult>,
    /// Fraction of meaningful query terms that matched entities (0.0..=1.0).
    pub entity_coverage: f32,
}

pub struct GraphSearcher {
    db: Arc<dyn Store>,
    config: RetrievalConfig,
}

impl GraphSearcher {
    pub fn new(db: Arc<dyn Store>, config: RetrievalConfig) -> Self {
        Self { db, config }
    }

    /// Run graph-based retrieval for the given query within a store.
    pub async fn search(
        &self,
        query: &str,
        store_id: &str,
        top_k: usize,
    ) -> Result<GraphSearchOutput> {
        // 1. Extract meaningful terms from query
        let terms = self.extract_terms(query);
        if terms.is_empty() {
            return Ok(GraphSearchOutput { results: vec![], entity_coverage: 0.0 });
        }

        // 2. Find matching entities
        let term_refs: Vec<&str> = terms.iter().map(|s| s.as_str()).collect();
        let matched_entities = self.db.search_entities_by_name(store_id, &term_refs).await?;
        let entity_coverage = matched_entities.len() as f32 / terms.len() as f32;
        // Clamp to 1.0 (more entity matches than terms is possible with prefix matches)
        let entity_coverage = entity_coverage.min(1.0);

        debug!(
            "Graph search: {} terms → {} entities matched (coverage: {:.2})",
            terms.len(), matched_entities.len(), entity_coverage
        );

        if matched_entities.is_empty() {
            return Ok(GraphSearchOutput { results: vec![], entity_coverage: 0.0 });
        }

        // 3. Get articles via MENTIONS edges
        let entity_ids: Vec<&str> = matched_entities.iter().map(|e| e.id.as_str()).collect();
        let mentioned_articles = self.db.list_articles_for_entities(&entity_ids).await?;

        // 4. Score articles: direct mention score
        let mut article_scores: HashMap<String, (f64, crate::store::Article)> = HashMap::new();
        for (article, confidence) in &mentioned_articles {
            let entry = article_scores
                .entry(article.id.clone())
                .or_insert_with(|| (0.0, article.clone()));
            entry.0 += confidence; // Sum confidence across matched entities
        }

        // 5. One-hop RELATED_TO traversal (if configured)
        if self.config.graph_hops >= 1 {
            let direct_article_ids: Vec<String> = article_scores.keys().cloned().collect();
            for aid in &direct_article_ids {
                if let Ok(related) = self.db.list_related_articles(aid).await {
                    for related_article in related {
                        if !article_scores.contains_key(&related_article.id) {
                            // Decay factor for one-hop results
                            let base_score = article_scores.get(aid).map(|s| s.0).unwrap_or(0.0);
                            let hop_score = base_score * 0.5;
                            article_scores
                                .entry(related_article.id.clone())
                                .or_insert_with(|| (hop_score, related_article));
                        }
                    }
                }
            }
        }

        // 6. Convert to ranked K2KResult list
        let mut scored: Vec<(f64, crate::store::Article)> = article_scores.into_values().collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        let results: Vec<K2KResult> = scored
            .into_iter()
            .enumerate()
            .map(|(rank, (score, article))| {
                let summary = if article.content.len() > 200 {
                    let end = (0..=200)
                        .rev()
                        .find(|&i| article.content.is_char_boundary(i))
                        .unwrap_or(0);
                    format!("{}...", &article.content[..end])
                } else {
                    article.content.clone()
                };
                K2KResult {
                    article_id: article.id.clone(),
                    store_id: article.store_id.clone(),
                    title: article.title,
                    summary,
                    content: article.content,
                    confidence: score as f32,
                    source_type: article.source_type,
                    tags: vec![],
                    metadata: serde_json::json!({
                        "search_type": "graph",
                        "graph_score": score,
                    }),
                    provenance: Some(ResultProvenance {
                        store_id: article.store_id,
                        store_type: "graph".into(),
                        original_rank: rank,
                        rrf_score: 0.0,
                    }),
                }
            })
            .collect();

        Ok(GraphSearchOutput { results, entity_coverage })
    }

    /// Extract meaningful query terms by removing stop words.
    fn extract_terms(&self, query: &str) -> Vec<String> {
        let expander = QueryExpander::new();
        // Use stop-word removal to get meaningful terms
        let cleaned = expander.expand(query);
        // The second variant (index 1) is stop-words-removed if it exists
        let meaningful = if cleaned.len() > 1 {
            &cleaned[1]
        } else {
            &cleaned[0]
        };
        meaningful
            .split_whitespace()
            .filter(|w| w.len() > 1)
            .map(|w| w.to_string())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_terms_removes_stop_words() {
        let config = RetrievalConfig::default();
        // We can't construct a real GraphSearcher without a Store, so test via QueryExpander
        let expander = QueryExpander::new();
        let variants = expander.expand("how to configure the database");
        // Second variant should be stop-word-free
        assert!(variants.len() > 1);
        assert!(!variants[1].contains("how"));
        assert!(!variants[1].contains("the"));
        assert!(variants[1].contains("configure"));
        assert!(variants[1].contains("database"));
    }
}
```

- [ ] **Step 2: Register the module in `src/retrieval/mod.rs`**

Replace the contents of `src/retrieval/mod.rs` with:

```rust
pub mod confidence;
pub mod expansion;
pub mod graph;
pub mod hybrid;
pub mod reranker;

pub use confidence::ConfidenceScorer;
pub use expansion::QueryExpander;
pub use graph::GraphSearcher;
pub use hybrid::HybridSearcher;
pub use reranker::Reranker;
```

- [ ] **Step 3: Run tests to verify compilation**

Run: `cargo test --lib -- retrieval::graph::tests 2>&1 | tail -5`
Expected: `test_extract_terms_removes_stop_words` passes.

- [ ] **Step 4: Commit**

```bash
git add src/retrieval/graph.rs src/retrieval/mod.rs
git commit -m "feat(retrieval): add GraphSearcher with entity matching and edge traversal"
```

---

### Task 4: Refactor RRF merge to N-way with adaptive weighting

**Files:**
- Modify: `src/retrieval/hybrid.rs`

- [ ] **Step 1: Write tests for `merge_signals`**

In `src/retrieval/hybrid.rs`, replace the existing `#[cfg(test)]` block (there isn't one currently, so add at the bottom):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::k2k::models::ResultProvenance;

    fn make_result(id: &str, store_id: &str) -> K2KResult {
        K2KResult {
            article_id: id.into(),
            store_id: store_id.into(),
            title: format!("Article {}", id),
            summary: String::new(),
            content: String::new(),
            confidence: 0.0,
            source_type: "local".into(),
            tags: vec![],
            metadata: serde_json::json!({}),
            provenance: Some(ResultProvenance {
                store_id: store_id.into(),
                store_type: "test".into(),
                original_rank: 0,
                rrf_score: 0.0,
            }),
        }
    }

    #[test]
    fn test_merge_signals_two_lists() {
        let vector = vec![make_result("a1", "s1"), make_result("a2", "s1")];
        let keyword = vec![make_result("a2", "s1"), make_result("a3", "s1")];
        let signals = vec![
            RankedSignal { results: vector, weight: 1.0 },
            RankedSignal { results: keyword, weight: 1.1 },
        ];
        let merged = merge_signals(signals, 10, 60.0);
        // a2 appears in both, should rank highest
        assert_eq!(merged[0].article_id, "a2");
        assert_eq!(merged.len(), 3);
    }

    #[test]
    fn test_merge_signals_three_lists_with_graph() {
        let vector = vec![make_result("a1", "s1"), make_result("a2", "s1")];
        let keyword = vec![make_result("a2", "s1"), make_result("a3", "s1")];
        let graph = vec![make_result("a3", "s1"), make_result("a4", "s1")];
        let signals = vec![
            RankedSignal { results: vector, weight: 1.0 },
            RankedSignal { results: keyword, weight: 1.1 },
            RankedSignal { results: graph, weight: 0.8 },
        ];
        let merged = merge_signals(signals, 10, 60.0);
        assert_eq!(merged.len(), 4); // a1, a2, a3, a4
        // a2 and a3 appear in 2 lists each, should rank above a1 and a4
        let top_two: Vec<&str> = merged[..2].iter().map(|r| r.article_id.as_str()).collect();
        assert!(top_two.contains(&"a2"));
        assert!(top_two.contains(&"a3"));
    }

    #[test]
    fn test_merge_signals_zero_weight_list_ignored() {
        let vector = vec![make_result("a1", "s1")];
        let graph = vec![make_result("a2", "s1")];
        let signals = vec![
            RankedSignal { results: vector, weight: 1.0 },
            RankedSignal { results: graph, weight: 0.0 },
        ];
        let merged = merge_signals(signals, 10, 60.0);
        // a2 should still appear but with 0 score from graph
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].article_id, "a1");
    }

    #[test]
    fn test_merge_signals_empty_inputs() {
        let signals: Vec<RankedSignal> = vec![];
        let merged = merge_signals(signals, 10, 60.0);
        assert!(merged.is_empty());
    }

    #[test]
    fn test_merge_signals_top_k_limit() {
        let long_list: Vec<K2KResult> = (0..20).map(|i| make_result(&format!("a{}", i), "s1")).collect();
        let signals = vec![RankedSignal { results: long_list, weight: 1.0 }];
        let merged = merge_signals(signals, 5, 60.0);
        assert_eq!(merged.len(), 5);
    }
}
```

- [ ] **Step 2: Add the `RankedSignal` struct and `merge_signals` function**

At the top of `src/retrieval/hybrid.rs`, after the existing imports, add:

```rust
/// A ranked list of results from a single retrieval signal (vector, FTS, graph)
/// with an associated weight for RRF fusion.
pub struct RankedSignal {
    pub results: Vec<K2KResult>,
    pub weight: f32,
}

/// Merge N ranked result lists using weighted Reciprocal Rank Fusion.
///
/// Each signal contributes `weight / (rrf_k + rank + 1)` per result.
/// Articles appearing in multiple signals accumulate scores.
pub fn merge_signals(
    signals: Vec<RankedSignal>,
    top_k: usize,
    rrf_k: f32,
) -> Vec<K2KResult> {
    use std::collections::HashMap;

    let mut scores: HashMap<String, (f32, K2KResult)> = HashMap::new();

    for signal in signals {
        for (rank, result) in signal.results.into_iter().enumerate() {
            let rrf_score = signal.weight / (rrf_k + rank as f32 + 1.0);
            let key = result.article_id.clone();
            let entry = scores.entry(key).or_insert_with(|| (0.0, result.clone()));
            entry.0 += rrf_score;
            if result.confidence > entry.1.confidence {
                entry.1 = result;
            }
        }
    }

    let mut results: Vec<(f32, K2KResult)> = scores.into_values().collect();
    results.sort_by(|a, b| {
        match b.0.partial_cmp(&a.0) {
            Some(ord) => ord,
            None => {
                if b.0.is_nan() { std::cmp::Ordering::Less }
                else { std::cmp::Ordering::Greater }
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
```

- [ ] **Step 3: Refactor `merge_hybrid` to use `merge_signals` internally**

Replace the existing `merge_hybrid` method body:

```rust
    /// Merge vector search results with keyword search results using RRF
    pub fn merge_hybrid(
        &self,
        vector_results: Vec<K2KResult>,
        keyword_results: Vec<K2KResult>,
        top_k: usize,
    ) -> Vec<K2KResult> {
        let signals = vec![
            RankedSignal { results: vector_results, weight: 1.0 },
            RankedSignal { results: keyword_results, weight: 1.1 },
        ];
        merge_signals(signals, top_k, RRF_K)
    }
```

- [ ] **Step 4: Run tests to verify**

Run: `cargo test --lib -- retrieval::hybrid::tests 2>&1 | tail -10`
Expected: All 5 new tests pass.

Also run existing merger tests to verify no regression:
Run: `cargo test --lib -- router::merger::tests 2>&1 | tail -5`
Expected: All 3 existing tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/retrieval/hybrid.rs
git commit -m "feat(retrieval): generalize RRF to N-way merge_signals with adaptive weighting"
```

---

### Task 5: Wire GraphSearcher into the retrieval pipeline

**Files:**
- Modify: `src/router/executor.rs`
- Modify: `src/router/mod.rs`

- [ ] **Step 1: Add `GraphSearcher` to `QueryExecutor`**

In `src/router/executor.rs`, update imports:

```rust
use crate::config::RetrievalConfig;
use crate::retrieval::{HybridSearcher, GraphSearcher};
use crate::retrieval::hybrid::{RankedSignal, merge_signals};
```

Update the `QueryExecutor` struct:

```rust
pub struct QueryExecutor {
    vectordb: Arc<VectorDB>,
    embedding_model: Arc<Mutex<EmbeddingModel>>,
    hybrid_searcher: Option<Arc<HybridSearcher>>,
    graph_searcher: Option<Arc<GraphSearcher>>,
    retrieval_config: RetrievalConfig,
}
```

Update the constructor:

```rust
impl QueryExecutor {
    pub fn new(
        vectordb: Arc<VectorDB>,
        embedding_model: Arc<Mutex<EmbeddingModel>>,
        hybrid_searcher: Option<Arc<HybridSearcher>>,
        graph_searcher: Option<Arc<GraphSearcher>>,
        retrieval_config: RetrievalConfig,
    ) -> Self {
        Self {
            vectordb,
            embedding_model,
            hybrid_searcher,
            graph_searcher,
            retrieval_config,
        }
    }
```

- [ ] **Step 2: Wire graph search into `execute` method**

In the `execute` method, after the hybrid merge block (`let final_results = if let Some(ref hybrid) ...`), replace the end of the per-store loop body with tri-signal merge:

```rust
            // If hybrid searcher is available, run keyword search
            let keyword_results = if let Some(ref hybrid) = self.hybrid_searcher {
                match hybrid.keyword_search(query, top_k).await {
                    Ok(kw) if !kw.is_empty() => {
                        debug!(
                            "Hybrid search: {} vector + {} keyword results for store {}",
                            vector_results.len(), kw.len(), store_id
                        );
                        kw
                    }
                    Ok(_) => vec![],
                    Err(e) => {
                        debug!("Keyword search failed (using vector only): {}", e);
                        vec![]
                    }
                }
            } else {
                vec![]
            };

            // Graph search (if available)
            let (graph_results, entity_coverage) = if let Some(ref graph) = self.graph_searcher {
                match graph.search(query, store_id, top_k).await {
                    Ok(output) => {
                        debug!(
                            "Graph search: {} results, coverage {:.2} for store {}",
                            output.results.len(), output.entity_coverage, store_id
                        );
                        (output.results, output.entity_coverage)
                    }
                    Err(e) => {
                        debug!("Graph search failed (continuing without): {}", e);
                        (vec![], 0.0)
                    }
                }
            } else {
                (vec![], 0.0)
            };

            // Build signals for N-way RRF merge
            let cfg = &self.retrieval_config;
            let graph_weight = cfg.graph_weight_max * entity_coverage;
            let mut signals = vec![
                RankedSignal { results: vector_results, weight: cfg.vector_weight },
            ];
            if !keyword_results.is_empty() {
                signals.push(RankedSignal { results: keyword_results, weight: cfg.keyword_weight });
            }
            if !graph_results.is_empty() && graph_weight > 0.0 {
                signals.push(RankedSignal { results: graph_results, weight: graph_weight });
            }

            let final_results = merge_signals(signals, top_k, cfg.rrf_k);
```

- [ ] **Step 3: Update `LocalRouter::new` to pass graph searcher and config**

In `src/router/mod.rs`, update imports:

```rust
use crate::config::RetrievalConfig;
use crate::retrieval::{ConfidenceScorer, GraphSearcher, HybridSearcher, QueryExpander, Reranker};
```

Update `LocalRouter::new` signature and body:

```rust
    pub fn new(
        db: Arc<dyn Store>,
        vectordb: Arc<VectorDB>,
        embedding_model: Arc<Mutex<EmbeddingModel>>,
        hybrid_searcher: Option<Arc<HybridSearcher>>,
        remote_executor: Option<Arc<RemoteQueryExecutor>>,
        retrieval_config: RetrievalConfig,
    ) -> Self {
        let graph_searcher = Some(Arc::new(GraphSearcher::new(db.clone(), retrieval_config.clone())));
        Self {
            classifier: ContextClassifier::new(),
            planner: QueryPlanner::new(db.clone()),
            executor: QueryExecutor::new(
                vectordb,
                embedding_model,
                hybrid_searcher,
                graph_searcher,
                retrieval_config,
            ),
            merger: ResultMerger::new(),
            reranker: Reranker::new(),
            query_expander: QueryExpander::new(),
            confidence_scorer: ConfidenceScorer::new(),
            remote_executor,
        }
    }
```

- [ ] **Step 4: Update `K2KServer::new` to pass `RetrievalConfig`**

In `src/k2k/server.rs`, update the `LocalRouter::new` call (around line 75):

```rust
        let router = Arc::new(LocalRouter::new(
            db.clone(),
            vectordb.clone(),
            embedding_model.clone(),
            hybrid_searcher,
            remote_executor,
            config.retrieval.clone(),
        ));
```

- [ ] **Step 5: Run the full test suite**

Run: `cargo test 2>&1 | tail -20`
Expected: All tests pass. The graph searcher returns empty results in tests without entity data, so existing behavior is preserved.

- [ ] **Step 6: Commit**

```bash
git add src/router/executor.rs src/router/mod.rs src/k2k/server.rs
git commit -m "feat(router): wire GraphSearcher into tri-signal retrieval pipeline"
```

---

### Task 6: Unify CLI `search` onto `LocalRouter`

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Update the `Search` command definition**

In `src/main.rs`, replace the `Search` variant in the `Commands` enum:

```rust
    /// Search articles (hybrid: vector + keyword + graph)
    Search {
        /// Search query
        query: String,

        /// Maximum results
        #[arg(short, long, default_value = "10")]
        limit: usize,

        /// Restrict to a specific store ID
        #[arg(long)]
        store: Option<String>,

        /// Show detailed provenance info (signal sources, RRF scores)
        #[arg(short, long)]
        verbose: bool,
    },
```

- [ ] **Step 2: Update the match arm for Search**

Replace the `Commands::Search` match arm:

```rust
            Commands::Search { query, limit, store, verbose } => {
                cmd_search(&query, limit, store.as_deref(), verbose).await?;
            }
```

- [ ] **Step 3: Rewrite `cmd_search` to use `LocalRouter`**

Replace the entire `cmd_search` function:

```rust
async fn cmd_search(query: &str, limit: usize, store_filter: Option<&str>, verbose: bool) -> Result<()> {
    info!("Searching for: {}", query);

    let cfg = config::load_config().await?;
    let db = open_store_or_bail(&cfg).await?;

    // Get owner user for the router
    let owner = db.get_owner_user().await?
        .ok_or_else(|| anyhow::anyhow!("No owner user found. Run `init` first."))?;

    // Initialize retrieval stack
    let registry = vectordb::quantizer::QuantizerRegistry::new();
    // Find default store to get quantizer version
    let stores = db.list_stores_for_user(&owner.id).await?;
    let default_store = stores.first()
        .ok_or_else(|| anyhow::anyhow!("No knowledge stores found. Create articles first."))?;
    let quantizer = registry.resolve(&default_store.quantizer_version)?;
    let vdb = std::sync::Arc::new(vectordb::VectorDB::open(quantizer).await?);
    let emb = embeddings::EmbeddingModel::new()?;
    let emb_arc = std::sync::Arc::new(tokio::sync::Mutex::new(emb));
    let hybrid = Some(std::sync::Arc::new(
        retrieval::HybridSearcher::new(db.clone()),
    ));

    let router = router::LocalRouter::new(
        db.clone(), vdb, emb_arc, hybrid, None, cfg.retrieval.clone(),
    );

    let response = router.route(query, &owner.id, store_filter, limit).await?;

    if response.results.is_empty() {
        println!("No results found for: {}", query);
        return Ok(());
    }

    println!("Found {} results ({}ms):\n", response.total_results, response.query_time_ms);
    for (i, result) in response.results.iter().enumerate() {
        println!("{}. [{:.2}] {}", i + 1, result.confidence, result.title);
        // Show summary snippet
        let snippet = if result.summary.len() > 150 {
            let end = (0..=150)
                .rev()
                .find(|&j| result.summary.is_char_boundary(j))
                .unwrap_or(0);
            format!("{}...", &result.summary[..end])
        } else {
            result.summary.clone()
        };
        if !snippet.is_empty() {
            println!("   {}", snippet);
        }
        if verbose {
            if let Some(ref prov) = result.provenance {
                println!("   via: {} (rank: {}, rrf: {:.4})", prov.store_type, prov.original_rank, prov.rrf_score);
            }
        }
        println!();
    }

    Ok(())
}
```

- [ ] **Step 4: Verify compilation**

Run: `cargo build 2>&1 | tail -5`
Expected: Compiles successfully.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat(cli): unify search command onto LocalRouter with tri-signal pipeline"
```

---

### Task 7: Add `graph` CLI debug command

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Add `Graph` command and `GraphAction` enum**

In `src/main.rs`, add to the `Commands` enum (after `ExtractEntities`):

```rust
    /// Inspect knowledge graph entities and connections
    Graph {
        #[command(subcommand)]
        action: GraphAction,
    },
```

Add the `GraphAction` enum (after `DedupReviewAction`):

```rust
#[derive(Subcommand)]
enum GraphAction {
    /// Show entity details, mentioning articles, and co-mentioned entities
    Entity {
        /// Entity name to search for
        name: String,

        /// Store ID (uses default store if omitted)
        #[arg(long)]
        store: Option<String>,
    },

    /// Show entities, related articles, and tags for an article
    Article {
        /// Article ID
        id: String,
    },

    /// Show aggregate graph statistics
    Stats {
        /// Store ID (uses default store if omitted)
        #[arg(long)]
        store: Option<String>,
    },
}
```

- [ ] **Step 2: Add the match arm and dispatch**

In the main match block, add:

```rust
            Commands::Graph { action } => {
                match action {
                    GraphAction::Entity { name, store } => {
                        cmd_graph_entity(&name, store.as_deref()).await?;
                    }
                    GraphAction::Article { id } => {
                        cmd_graph_article(&id).await?;
                    }
                    GraphAction::Stats { store } => {
                        cmd_graph_stats(store.as_deref()).await?;
                    }
                }
            }
```

- [ ] **Step 3: Implement `cmd_graph_entity`**

```rust
async fn cmd_graph_entity(name: &str, store_filter: Option<&str>) -> Result<()> {
    let cfg = config::load_config().await?;
    let db = open_store_or_bail(&cfg).await?;

    let store_id = match store_filter {
        Some(id) => id.to_string(),
        None => {
            let owner = db.get_owner_user().await?
                .ok_or_else(|| anyhow::anyhow!("No owner user found"))?;
            let stores = db.list_stores_for_user(&owner.id).await?;
            stores.first()
                .ok_or_else(|| anyhow::anyhow!("No stores found"))?
                .id.clone()
        }
    };

    let entities = db.search_entities_by_name(&store_id, &[name]).await?;
    if entities.is_empty() {
        println!("No entities found matching \"{}\"", name);
        return Ok(());
    }

    for entity in &entities {
        println!("Entity: {} ({})", entity.name, entity.entity_type);
        println!("  Mentions: {} articles", entity.mention_count);
        if let Some(ref desc) = entity.description {
            println!("  Description: \"{}\"", desc);
        }

        // Articles mentioning this entity
        let articles = db.list_articles_for_entity(&entity.id).await?;
        if !articles.is_empty() {
            println!("\n  Top articles:");
            for (i, article) in articles.iter().take(10).enumerate() {
                println!("    {}. {}", i + 1, article.title);
            }
        }

        // Co-mentioned entities
        let co = db.list_co_mentioned_entities(&entity.id).await?;
        if !co.is_empty() {
            println!("\n  Related entities (co-mentioned):");
            for (co_entity, count) in co.iter().take(10) {
                println!("    {}:{} ({} shared articles)", co_entity.entity_type, co_entity.name, count);
            }
        }

        println!();
    }

    Ok(())
}
```

- [ ] **Step 4: Implement `cmd_graph_article`**

```rust
async fn cmd_graph_article(article_id: &str) -> Result<()> {
    let cfg = config::load_config().await?;
    let db = open_store_or_bail(&cfg).await?;

    let article = db.get_article(article_id).await?
        .ok_or_else(|| anyhow::anyhow!("Article '{}' not found", article_id))?;

    println!("Article: {}", article.title);
    println!("  ID: {}", article.id);
    println!("  Store: {}", article.store_id);

    // Entities
    let entities = db.list_entities_for_article(article_id).await?;
    if entities.is_empty() {
        println!("\n  Entities: (none — run extract-entities to populate)");
    } else {
        println!("\n  Entities mentioned:");
        for entity in &entities {
            println!("    {}:{} (mentions: {})", entity.entity_type, entity.name, entity.mention_count);
        }
    }

    // Related articles
    let related = db.list_related_articles(article_id).await?;
    if !related.is_empty() {
        println!("\n  Related articles (via RELATED_TO):");
        for r in related.iter().take(10) {
            println!("    {} ({})", r.title, r.id);
        }
    }

    // Tags
    let tags = db.list_tags_for_article(article_id).await?;
    if !tags.is_empty() {
        let tag_names: Vec<&str> = tags.iter().map(|t| t.name.as_str()).collect();
        println!("\n  Tags: {}", tag_names.join(", "));
    }

    Ok(())
}
```

- [ ] **Step 5: Implement `cmd_graph_stats`**

```rust
async fn cmd_graph_stats(store_filter: Option<&str>) -> Result<()> {
    let cfg = config::load_config().await?;
    let db = open_store_or_bail(&cfg).await?;

    let store_id = match store_filter {
        Some(id) => id.to_string(),
        None => {
            let owner = db.get_owner_user().await?
                .ok_or_else(|| anyhow::anyhow!("No owner user found"))?;
            let stores = db.list_stores_for_user(&owner.id).await?;
            stores.first()
                .ok_or_else(|| anyhow::anyhow!("No stores found"))?
                .id.clone()
        }
    };

    let store = db.get_store(&store_id).await?
        .ok_or_else(|| anyhow::anyhow!("Store '{}' not found", store_id))?;

    println!("Store: {} ({})", store.name, store.id);

    // Entity counts by type
    let counts = db.count_entities_by_type(&store_id).await?;
    let total_entities: usize = counts.values().sum();
    if total_entities == 0 {
        println!("  Entities: 0 (run extract-entities to populate)");
    } else {
        let type_breakdown: Vec<String> = counts.iter()
            .map(|(t, c)| format!("{} {}", c, t))
            .collect();
        println!("  Entities: {} ({})", total_entities, type_breakdown.join(", "));
    }

    // Article counts
    let all_articles = db.list_articles_for_store(&store_id).await?;
    let without_mentions = db.list_articles_without_mentions(&store_id).await?;
    let with_mentions = all_articles.len() - without_mentions.len();
    println!("  Articles with extractions: {}/{}", with_mentions, all_articles.len());

    // Average entities per article
    if with_mentions > 0 {
        let avg = total_entities as f64 / with_mentions as f64;
        println!("  Avg entities per article: {:.1}", avg);
    }

    Ok(())
}
```

- [ ] **Step 6: Verify compilation**

Run: `cargo build 2>&1 | tail -5`
Expected: Compiles successfully.

- [ ] **Step 7: Commit**

```bash
git add src/main.rs
git commit -m "feat(cli): add graph subcommand for entity/article/stats inspection"
```

---

### Task 8: Integration test — tri-signal search end-to-end

**Files:**
- Modify: `src/store/mod.rs` (add integration test in `p3_integration_tests` module)

- [ ] **Step 1: Write the integration test**

In `src/store/mod.rs`, inside the `#[cfg(test)] mod p3_integration_tests` block, add:

```rust
    #[tokio::test]
    async fn test_graph_search_integration() {
        let s = fixture().await;
        let ts = now();

        // Create two articles
        s.create_article(&Article {
            id: "a1".into(), store_id: "s1".into(), title: "Rust Async Programming".into(),
            content: "Rust provides powerful async capabilities using Tokio runtime".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "h1".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_article(&Article {
            id: "a2".into(), store_id: "s1".into(), title: "Go Concurrency".into(),
            content: "Go uses goroutines for concurrent programming".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "h2".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_article(&Article {
            id: "a3".into(), store_id: "s1".into(), title: "Tokio Internals".into(),
            content: "Deep dive into how Tokio scheduler works".into(),
            source_type: "user".into(), source_id: String::new(), content_hash: "h3".into(),
            tags: serde_json::json!([]), embedded_at: None,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        // Create entities
        s.create_entity(&Entity {
            id: "tool:rust".into(), name: "Rust".into(), entity_type: "tool".into(),
            description: Some("Systems programming language".into()), store_id: "s1".into(),
            mention_count: 2, created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        s.create_entity(&Entity {
            id: "tool:tokio".into(), name: "Tokio".into(), entity_type: "tool".into(),
            description: Some("Async runtime for Rust".into()), store_id: "s1".into(),
            mention_count: 2, created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        // Create MENTIONS edges
        s.create_mentions_edge("a1", "tool:rust", "Rust provides", 0.95).await.unwrap();
        s.create_mentions_edge("a1", "tool:tokio", "using Tokio", 0.90).await.unwrap();
        s.create_mentions_edge("a3", "tool:tokio", "Tokio scheduler", 0.92).await.unwrap();

        // Create RELATED_TO edge (a1 and a3 share tokio)
        s.create_or_update_related_to_edge("a1", "a3", 1, 0.5).await.unwrap();

        // Test GraphSearcher
        let config = crate::config::RetrievalConfig::default();
        let db: std::sync::Arc<dyn Store> = std::sync::Arc::new(s);
        let searcher = crate::retrieval::GraphSearcher::new(db, config);

        // Search for "Rust" — should find a1 (direct mention)
        let output = searcher.search("Rust", "s1", 10).await.unwrap();
        assert!(!output.results.is_empty());
        assert!(output.entity_coverage > 0.0);
        assert!(output.results.iter().any(|r| r.article_id == "a1"));

        // Search for "Tokio" — should find a1 and a3 (both mention tokio)
        let output = searcher.search("Tokio", "s1", 10).await.unwrap();
        assert!(output.results.len() >= 2);
        let article_ids: Vec<&str> = output.results.iter().map(|r| r.article_id.as_str()).collect();
        assert!(article_ids.contains(&"a1"));
        assert!(article_ids.contains(&"a3"));

        // Search for "Go" — should find nothing (no entity for Go)
        let output = searcher.search("Go", "s1", 10).await.unwrap();
        assert!(output.results.is_empty());
        assert_eq!(output.entity_coverage, 0.0);
    }
```

- [ ] **Step 2: Run the test**

Run: `cargo test --lib -- p3_integration_tests::test_graph_search_integration 2>&1 | tail -10`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/store/mod.rs
git commit -m "test(retrieval): add tri-signal graph search integration test"
```

---

### Task 9: Final full test suite run and cleanup

- [ ] **Step 1: Run the complete test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: All tests pass. No regressions.

- [ ] **Step 2: Run clippy for lint check**

Run: `cargo clippy --lib --bins --tests 2>&1 | grep -E "warning|error" | grep -v "lance\|patches" | head -20`
Expected: No new warnings from `knowledge-nexus-agent` code.

- [ ] **Step 3: Fix any issues found in steps 1-2**

- [ ] **Step 4: Final commit if any fixes needed**

```bash
git add -A
git commit -m "fix: address clippy warnings and test issues from P4 implementation"
```
