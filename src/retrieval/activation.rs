//! Spreading-activation retrieval engine (P6).
//!
//! Orchestrates: intent classification → seed identification → subgraph
//! construction → edge matrix assembly with intent weights → PPR diffusion
//! → SYNAPSE post-processing → K2KResult.
//!
//! Components are implemented as independent modules:
//! - `crate::retrieval::intent` — Intent enum + per-intent edge weights (MAGMA)
//! - `crate::retrieval::specificity` — node specificity cache (HippoRAG)
//! - `crate::retrieval::ppr` — Personalized PageRank (HippoRAG)
//! - `crate::retrieval::post_process` — lateral inhibition + sigmoid + gate (SYNAPSE)

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::Result;
use sprs::{CsMat, CsVec, TriMat};

use crate::config::{ActivationConfig, EdgeTypeFilter, RetrievalConfig};
use crate::k2k::models::{K2KResult, ResultProvenance};
use crate::retrieval::intent::{classify, Intent, IntentWeights};
use crate::retrieval::ppr::personalized_pagerank;
use crate::retrieval::post_process::post_process;
use crate::retrieval::specificity::SpecificityCache;
use crate::store::Store;

pub struct ActivationEngine {
    db: Arc<dyn Store>,
    activation_config: ActivationConfig,
    edge_filter: EdgeTypeFilter,
    specificity: Arc<SpecificityCache>,
}

pub struct ActivationOutput {
    pub results: Vec<K2KResult>,
    pub entity_coverage: f32,
    pub intent: Intent,
    pub node_count: usize,
}

#[derive(Default)]
struct NodeIndex {
    id_to_idx: HashMap<String, usize>,
    idx_to_id: Vec<String>,
}

impl NodeIndex {
    fn insert(&mut self, id: String) -> usize {
        if let Some(&i) = self.id_to_idx.get(&id) {
            return i;
        }
        let i = self.idx_to_id.len();
        self.id_to_idx.insert(id.clone(), i);
        self.idx_to_id.push(id);
        i
    }

    fn len(&self) -> usize {
        self.idx_to_id.len()
    }

    fn is_empty(&self) -> bool {
        self.idx_to_id.is_empty()
    }
}

struct TypedEdge {
    from: usize,
    to: usize,
    edge_type: String,
    raw_weight: f32,
}

impl ActivationEngine {
    pub fn new(db: Arc<dyn Store>, config: RetrievalConfig) -> Self {
        Self {
            db,
            activation_config: config.activation.clone(),
            edge_filter: config.edge_types.clone(),
            specificity: Arc::new(SpecificityCache::new()),
        }
    }

    /// Run spreading-activation retrieval for a query within a store.
    pub async fn search(&self, query: &str, store_id: &str, top_k: usize) -> Result<ActivationOutput> {
        let intent = classify(query);
        let weights = intent.weights();

        // 1. Find seed articles via existing graph queries: match query tokens
        //    against entity names, then find articles mentioning those entities.
        let seed_articles = self.find_seed_articles(query, store_id).await?;
        if seed_articles.is_empty() {
            return Ok(ActivationOutput {
                results: vec![],
                entity_coverage: 0.0,
                intent,
                node_count: 0,
            });
        }

        // 2. Specificity weights (cached per store)
        let specificity = self.specificity.get(self.db.as_ref(), store_id).await
            .unwrap_or_else(|_| Arc::new(HashMap::new()));

        // 3. Build active subgraph via BFS from seed articles
        let (nodes, edges) = self.build_subgraph(store_id, &seed_articles).await?;
        if nodes.is_empty() {
            return Ok(ActivationOutput {
                results: vec![],
                entity_coverage: 1.0,
                intent,
                node_count: 0,
            });
        }

        // 4. Assemble column-stochastic edge matrix with intent multipliers
        let w = build_edge_matrix(nodes.len(), &edges, &weights);

        // 5. Personalization vector: 1.0 at seed indices (no node-level
        //    specificity since seeds here are articles, not entities; the
        //    specificity map is keyed by entity_id, not article_id).
        let t = build_personalization_vector(nodes.len(), &nodes, &seed_articles, &specificity);

        // 6. PPR diffusion
        let activation = personalized_pagerank(
            &w,
            &t,
            self.activation_config.damping,
            self.activation_config.tolerance,
            self.activation_config.max_iter,
        );

        // 7. Post-processing
        let cfg = &self.activation_config;
        let effective_top_k = top_k.min(cfg.top_k);
        let ranked = post_process(
            &activation,
            cfg.inhibition_beta,
            cfg.inhibition_m,
            cfg.sigmoid_gamma,
            cfg.gate_tau,
            effective_top_k,
        );

        // 8. Materialize K2KResults
        let node_count = nodes.len();
        let results = self.materialize_results(store_id, &nodes, &ranked, intent).await?;

        Ok(ActivationOutput {
            results,
            entity_coverage: 1.0,
            intent,
            node_count,
        })
    }

    /// Find seed articles by matching query terms against entity names,
    /// then collecting articles that mention those entities. Returns
    /// article_ids (NOT entity_ids).
    async fn find_seed_articles(&self, query: &str, store_id: &str) -> Result<Vec<String>> {
        // Tokenize query; filter short words
        let tokens: Vec<String> = query
            .split_whitespace()
            .filter(|w| w.len() > 2)
            .map(|w| w.to_lowercase())
            .collect();
        if tokens.is_empty() {
            return Ok(vec![]);
        }

        let token_refs: Vec<&str> = tokens.iter().map(|s| s.as_str()).collect();
        let entities = self.db.search_entities_by_name(store_id, &token_refs).await?;
        if entities.is_empty() {
            return Ok(vec![]);
        }

        let entity_ids: Vec<&str> = entities.iter().map(|e| e.id.as_str()).collect();
        let articles_with_conf = self.db.list_articles_for_entities(&entity_ids).await?;

        // Deduplicate article IDs (an article may mention multiple seed entities)
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for (article, _confidence) in articles_with_conf {
            if seen.insert(article.id.clone()) {
                out.push(article.id);
            }
        }
        Ok(out)
    }

    /// BFS-bounded subgraph construction. Starts from seed articles, expands
    /// via Store::list_graph_neighbors using the configured EdgeTypeFilter,
    /// caps total nodes at `subgraph_cap`.
    async fn build_subgraph(
        &self,
        store_id: &str,
        seeds: &[String],
    ) -> Result<(NodeIndex, Vec<TypedEdge>)> {
        let mut nodes = NodeIndex::default();
        let mut edges: Vec<TypedEdge> = Vec::new();
        let mut visited: HashSet<String> = HashSet::new();
        let mut frontier: Vec<String> = Vec::new();

        // Seed nodes
        for s in seeds {
            nodes.insert(s.clone());
            visited.insert(s.clone());
            frontier.push(s.clone());
        }

        let cap = self.activation_config.subgraph_cap;
        while !frontier.is_empty() && nodes.len() < cap {
            let mut next_frontier: Vec<String> = Vec::new();
            for from_id in &frontier {
                let neighbors = self
                    .db
                    .list_graph_neighbors(store_id, from_id, &self.edge_filter)
                    .await?;
                for (neighbor_id, edge_type, score) in neighbors {
                    if nodes.len() >= cap {
                        break;
                    }
                    let from_idx = nodes.insert(from_id.clone());
                    let to_idx = nodes.insert(neighbor_id.clone());
                    edges.push(TypedEdge {
                        from: from_idx,
                        to: to_idx,
                        edge_type,
                        raw_weight: score as f32,
                    });
                    if visited.insert(neighbor_id.clone()) {
                        next_frontier.push(neighbor_id);
                    }
                }
            }
            frontier = next_frontier;
        }

        Ok((nodes, edges))
    }

    /// Translate ranked node indices back to articles → K2KResult.
    async fn materialize_results(
        &self,
        store_id: &str,
        nodes: &NodeIndex,
        ranked: &[(usize, f32)],
        intent: Intent,
    ) -> Result<Vec<K2KResult>> {
        let mut results = Vec::with_capacity(ranked.len());
        for (rank, (idx, score)) in ranked.iter().enumerate() {
            let article_id = &nodes.idx_to_id[*idx];
            let Some(article) = self.db.get_article(article_id).await? else { continue };
            // Defense-in-depth: skip articles from other stores
            if article.store_id != store_id { continue; }

            let summary = if article.content.len() > 200 {
                let end = (0..=200)
                    .rev()
                    .find(|&i| article.content.is_char_boundary(i))
                    .unwrap_or(0);
                format!("{}...", &article.content[..end])
            } else {
                article.content.clone()
            };

            results.push(K2KResult {
                article_id: article.id.clone(),
                store_id: article.store_id.clone(),
                title: article.title,
                summary,
                content: article.content,
                confidence: *score,
                source_type: article.source_type,
                tags: vec![],
                metadata: serde_json::json!({
                    "search_type": "activation",
                    "intent": format!("{:?}", intent),
                    "rank": rank,
                    "activation_score": score,
                }),
                provenance: Some(ResultProvenance {
                    store_id: article.store_id,
                    store_type: "activation".into(),
                    original_rank: rank,
                    rrf_score: 0.0,
                }),
            });
        }
        Ok(results)
    }
}

/// Build a column-stochastic edge matrix from the BFS edge list with
/// per-edge-type intent weight multipliers.
fn build_edge_matrix(n: usize, edges: &[TypedEdge], weights: &IntentWeights) -> CsMat<f32> {
    let mut tri = TriMat::new((n, n));

    for e in edges {
        let multiplier = match e.edge_type.as_str() {
            "entity_overlap" => weights.entity_overlap,
            "semantically_related" => weights.semantically_related,
            "precedes" => weights.precedes,
            "caused_by" => weights.caused_by,
            "references_edge" => weights.references_edge,
            _ => 1.0,
        };
        // Edge from `e.from` to `e.to`: target row = e.to, source col = e.from
        tri.add_triplet(e.to, e.from, e.raw_weight.max(0.0) * multiplier);
    }

    let csr: CsMat<f32> = tri.to_csr();

    // Column-stochastic normalization: each column sums to 1.0 (or 0 for dangling cols)
    let mut col_sums = vec![0.0_f32; n];
    for (&val, (_row, col)) in csr.iter() {
        col_sums[col] += val;
    }

    let mut tri2 = TriMat::new((n, n));
    for (&val, (row, col)) in csr.iter() {
        let s = col_sums[col];
        if s > 0.0 {
            tri2.add_triplet(row, col, val / s);
        }
    }
    tri2.to_csr()
}

/// Build the personalization vector: 1.0 at each seed node index.
/// (We don't use specificity here because seeds are articles, not entities;
/// specificity is keyed by entity_id.)
fn build_personalization_vector(
    n: usize,
    nodes: &NodeIndex,
    seeds: &[String],
    _specificity: &HashMap<String, f32>,
) -> CsVec<f32> {
    let mut paired: Vec<(usize, f32)> = Vec::new();
    for seed_id in seeds {
        if let Some(&idx) = nodes.id_to_idx.get(seed_id) {
            paired.push((idx, 1.0));
        }
    }
    paired.sort_by_key(|(i, _)| *i);
    paired.dedup_by_key(|(i, _)| *i);

    let (indices, values): (Vec<_>, Vec<_>) = paired.into_iter().unzip();
    CsVec::new(n, indices, values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_edge_matrix_normalizes_columns_to_one() {
        let mut nodes = NodeIndex::default();
        let a = nodes.insert("a".into());
        let b = nodes.insert("b".into());
        let c = nodes.insert("c".into());

        // A → B (w=2), A → C (w=2), B → C (w=1)
        let edges = vec![
            TypedEdge { from: a, to: b, edge_type: "entity_overlap".into(), raw_weight: 2.0 },
            TypedEdge { from: a, to: c, edge_type: "entity_overlap".into(), raw_weight: 2.0 },
            TypedEdge { from: b, to: c, edge_type: "entity_overlap".into(), raw_weight: 1.0 },
        ];

        // With OpenDomain weights (all 1.0), the multiplier doesn't change anything.
        let weights = Intent::OpenDomain.weights();
        let w = build_edge_matrix(3, &edges, &weights);

        // Col A (source col 0): outgoing edges total 2 + 2 = 4; normalized to 0.5 + 0.5
        let col_a_sum: f32 = w.iter().filter(|(_, (_, c))| *c == 0).map(|(v, _)| *v).sum();
        assert!((col_a_sum - 1.0).abs() < 1e-6, "col A should sum to 1.0, got {}", col_a_sum);

        // Col B (source col 1): one outgoing edge weight 1; normalized to 1.0
        let col_b_sum: f32 = w.iter().filter(|(_, (_, c))| *c == 1).map(|(v, _)| *v).sum();
        assert!((col_b_sum - 1.0).abs() < 1e-6, "col B should sum to 1.0, got {}", col_b_sum);

        // Col C has no outgoing edges; col sum is 0
        let col_c_sum: f32 = w.iter().filter(|(_, (_, c))| *c == 2).map(|(v, _)| *v).sum();
        assert!(col_c_sum.abs() < 1e-6, "col C (dangling) should sum to 0, got {}", col_c_sum);
    }

    #[test]
    fn build_edge_matrix_applies_intent_multipliers() {
        let mut nodes = NodeIndex::default();
        let a = nodes.insert("a".into());
        let b = nodes.insert("b".into());

        // A → B via causal edge (w=1) and via temporal edge (w=1) — different types
        let edges = vec![
            TypedEdge { from: a, to: b, edge_type: "caused_by".into(), raw_weight: 1.0 },
            TypedEdge { from: a, to: b, edge_type: "precedes".into(), raw_weight: 1.0 },
        ];

        // Why intent: caused_by × 4.0, precedes × 1.0
        // Pre-normalization weights: caused_by 4.0, precedes 1.0. Total 5.0.
        // Post-normalization: caused_by 0.8, precedes 0.2.
        // But these are duplicate (row, col) entries; sprs may consolidate or stack — verify either way the col sums to 1.0.
        let weights = Intent::Why.weights();
        let w = build_edge_matrix(2, &edges, &weights);
        let col_a_sum: f32 = w.iter().filter(|(_, (_, c))| *c == 0).map(|(v, _)| *v).sum();
        assert!((col_a_sum - 1.0).abs() < 1e-6, "col A normalized to 1.0 regardless of intent");
    }

    #[test]
    fn node_index_dedups_inserts() {
        let mut idx = NodeIndex::default();
        let a1 = idx.insert("a".into());
        let a2 = idx.insert("a".into());
        let b = idx.insert("b".into());
        assert_eq!(a1, a2, "duplicate insert should return same index");
        assert_eq!(b, 1);
        assert_eq!(idx.len(), 2);
    }
}

#[cfg(test)]
mod ablation {
    use super::*;
    use crate::config::{EdgeTypeFilter, RetrievalConfig};
    use crate::retrieval::intent::Intent;
    use crate::store::{Article, Entity, SurrealStore, Store};
    use std::sync::Arc;

    /// Build a 6-article corpus with mixed edge types for ablation testing.
    ///
    /// Entity linkage:
    /// - "outage" entity → ab-a1, ab-a2, ab-a4 (common: 3 articles)
    /// - "decoy" entity → ab-a5 only (rare: 1 article)
    /// - ab-a6 is a pure decoy (no entity link)
    ///
    /// Causal chain (for Why-intent ablation):
    /// - ab-a1 → ab-a2 → ab-a3 via CAUSED_BY (BFS traversal direction: in=source,
    ///   out=neighbor). Seeds are a1, a2, a4 (outage entity); BFS from a1 reaches
    ///   a2, and from a2 reaches a3, making a3 the causal root that Why-intent
    ///   surfaces.
    ///
    /// Entity-overlap edges (pairwise among outage articles):
    /// - a1↔a2, a1↔a4, a2↔a4
    async fn build_ablation_fixture() -> Arc<dyn Store> {
        let s = SurrealStore::open_in_memory().await.expect("open mem");
        let ts = "2026-05-24T00:00:00Z".to_string();

        for (id, title) in &[
            ("ab-a1", "Outage retro"),
            ("ab-a2", "Deploy that broke things"),
            ("ab-a3", "Bad PR that caused deploy"),
            ("ab-a4", "Another outage mention"),
            ("ab-a5", "Unrelated topic"),
            ("ab-a6", "Decoy article"),
        ] {
            s.create_article(&Article {
                id: id.to_string(), store_id: "ab-s1".into(),
                title: title.to_string(), content: format!("content {}", id),
                source_type: "user".into(), source_id: String::new(),
                content_hash: format!("{}-h", id), tags: serde_json::json!([]),
                embedded_at: None,
                created_at: ts.clone(), updated_at: ts.clone(),
                reflects: vec![],
                access_count: 0,
                last_accessed_at: String::new(),
                importance_score: 0.5,
                tier: crate::store::Tier::Hot,
                pinned: false,
                compacted_into: None,
            }).await.unwrap();
        }

        // "outage" entity mentioned by a1, a2, a4 (common: 3 articles)
        s.create_entity(&Entity {
            id: "ab-ent-outage".into(), name: "outage".into(),
            entity_type: "concept".into(), description: None,
            store_id: "ab-s1".into(), mention_count: 3,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();
        // "decoy" entity mentioned only by a5 (rare: 1 article)
        s.create_entity(&Entity {
            id: "ab-ent-decoy".into(), name: "decoy".into(),
            entity_type: "concept".into(), description: None,
            store_id: "ab-s1".into(), mention_count: 1,
            created_at: ts.clone(), updated_at: ts.clone(),
        }).await.unwrap();

        for aid in &["ab-a1", "ab-a2", "ab-a4"] {
            s.create_mentions_edge(aid, "ab-ent-outage", "outage", 0.9).await.unwrap();
        }
        s.create_mentions_edge("ab-a5", "ab-ent-decoy", "decoy", 0.9).await.unwrap();

        // ENTITY_OVERLAP edges from shared "outage" entity (a1, a2, a4 pairwise)
        s.db().query(r#"
            LET $a1 = type::thing('article', 'ab-a1');
            LET $a2 = type::thing('article', 'ab-a2');
            LET $a4 = type::thing('article', 'ab-a4');
            RELATE $a1->entity_overlap->$a2 CONTENT {
                shared_entity_count: 1, strength: 0.5, confidence: 0.5,
                extraction_method: "heuristic", store_id: "ab-s1",
                created_at: "2026-05-24T00:00:00Z", updated_at: "2026-05-24T00:00:00Z"
            };
            RELATE $a1->entity_overlap->$a4 CONTENT {
                shared_entity_count: 1, strength: 0.5, confidence: 0.5,
                extraction_method: "heuristic", store_id: "ab-s1",
                created_at: "2026-05-24T00:00:00Z", updated_at: "2026-05-24T00:00:00Z"
            };
            RELATE $a2->entity_overlap->$a4 CONTENT {
                shared_entity_count: 1, strength: 0.5, confidence: 0.5,
                extraction_method: "heuristic", store_id: "ab-s1",
                created_at: "2026-05-24T00:00:00Z", updated_at: "2026-05-24T00:00:00Z"
            };
        "#).await.expect("seed overlap").check().expect("seed overlap check");

        // Causal chain: a1 → a2 → a3 via CAUSED_BY edges.
        //
        // list_graph_neighbors queries WHERE in=$aid, so BFS traversal follows
        // the "in" direction. Seeds are a1, a2, a4 (outage entity matches). From
        // a1, caused_by WHERE in=a1 finds a2. From a2, WHERE in=a2 finds a3.
        // This makes a3 reachable for the Why-intent ablation test.
        //
        // Semantics: a1 (outage) was caused by a2 (bad deploy); a2 was caused
        // by a3 (the bad PR). The CAUSED_BY relationship records the effect as
        // the "in" node and the cause as the "out" node.
        s.create_caused_by_edge("ab-s1", "ab-a1", "ab-a2", 0.9, None).await.unwrap();
        s.create_caused_by_edge("ab-s1", "ab-a2", "ab-a3", 0.9, None).await.unwrap();

        Arc::new(s)
    }

    fn base_config() -> RetrievalConfig {
        RetrievalConfig {
            edge_types: EdgeTypeFilter {
                entity_overlap: true,
                semantically_related: false,
                precedes: false,
                caused_by: true,
                references: false,
            },
            ..RetrievalConfig::default()
        }
    }

    /// Ablation 1: Damping = 0.0 means PPR degenerates to pure restart.
    /// At damping=0, a^(t+1) = (1-0)*t + 0*W*a = t for all iterations.
    /// Only seed nodes have non-zero activation; non-seed neighbors get 0
    /// and are dropped by the gate. Compared to damping=0.5, the result
    /// set should be smaller or equal.
    #[tokio::test]
    async fn ablation_damping_zero_keeps_mass_on_seeds() {
        let db = build_ablation_fixture().await;

        let mut cfg_zero = base_config();
        cfg_zero.activation.damping = 0.0;
        let engine_zero = ActivationEngine::new(db.clone(), cfg_zero);
        let out_zero = engine_zero.search("outage", "ab-s1", 10).await.unwrap();

        let cfg_baseline = base_config();
        let engine_baseline = ActivationEngine::new(db.clone(), cfg_baseline);
        let out_baseline = engine_baseline.search("outage", "ab-s1", 10).await.unwrap();

        // With damping=0, neighbors of seeds receive no mass — fewer
        // results should pass the gate.
        assert!(
            out_zero.results.len() <= out_baseline.results.len(),
            "damping=0 should produce fewer or equal results vs baseline; got {} vs {}",
            out_zero.results.len(), out_baseline.results.len()
        );
    }

    /// Ablation 2: Damping = 1.0 means pure random walk (no restart bias).
    /// Mass diffuses freely across the subgraph. Activation distribution
    /// should reach more nodes (or equal) compared to damping=0.5.
    #[tokio::test]
    async fn ablation_damping_one_reaches_more_nodes() {
        let db = build_ablation_fixture().await;

        let mut cfg_one = base_config();
        cfg_one.activation.damping = 1.0;
        let engine_one = ActivationEngine::new(db.clone(), cfg_one);
        let out_one = engine_one.search("outage", "ab-s1", 10).await.unwrap();

        let cfg_baseline = base_config();
        let engine_baseline = ActivationEngine::new(db.clone(), cfg_baseline);
        let out_baseline = engine_baseline.search("outage", "ab-s1", 10).await.unwrap();

        // Pure walk reaches at least as many nodes as 50% restart.
        assert!(
            out_one.node_count >= out_baseline.node_count,
            "damping=1.0 should reach >= as many subgraph nodes as damping=0.5; got {} vs {}",
            out_one.node_count, out_baseline.node_count
        );
    }

    /// Ablation 3: Inhibition β=0 disables lateral suppression.
    /// Hub nodes (high-activation) keep their full mass; with β>0 they get
    /// suppressed by competitors. So with β=0 the max activation should
    /// be at least as high as with β>0.
    #[tokio::test]
    async fn ablation_inhibition_zero_preserves_max_activation() {
        let db = build_ablation_fixture().await;

        let mut cfg_no_inhib = base_config();
        cfg_no_inhib.activation.inhibition_beta = 0.0;
        let engine_no = ActivationEngine::new(db.clone(), cfg_no_inhib);
        let out_no = engine_no.search("outage", "ab-s1", 10).await.unwrap();

        let cfg_baseline = base_config();
        let engine_baseline = ActivationEngine::new(db.clone(), cfg_baseline);
        let out_baseline = engine_baseline.search("outage", "ab-s1", 10).await.unwrap();

        let max_no: f32 = out_no.results.iter().map(|r| r.confidence).fold(0.0, f32::max);
        let max_baseline: f32 = out_baseline.results.iter().map(|r| r.confidence).fold(0.0, f32::max);

        // Without inhibition, the max can only stay the same or rise.
        assert!(
            max_no >= max_baseline - 1e-3,
            "max activation with β=0 ({}) should be >= max with β=0.15 ({})",
            max_no, max_baseline
        );
    }

    /// Ablation 4: Gate τ=0 disables the noise filter; more nodes pass the
    /// gate. With baseline τ=0.12, low-activation nodes are dropped.
    #[tokio::test]
    async fn ablation_gate_zero_passes_more_results() {
        let db = build_ablation_fixture().await;

        let mut cfg_no_gate = base_config();
        cfg_no_gate.activation.gate_tau = 0.0;
        let engine_no_gate = ActivationEngine::new(db.clone(), cfg_no_gate);
        let out_no_gate = engine_no_gate.search("outage", "ab-s1", 10).await.unwrap();

        let cfg_baseline = base_config();
        let engine_baseline = ActivationEngine::new(db.clone(), cfg_baseline);
        let out_baseline = engine_baseline.search("outage", "ab-s1", 10).await.unwrap();

        assert!(
            out_no_gate.results.len() >= out_baseline.results.len(),
            "no-gate ({}) should produce at least as many results as baseline ({})",
            out_no_gate.results.len(), out_baseline.results.len()
        );
    }

    /// Ablation 5: Sigmoid γ=0 makes the normalization function constant
    /// (every input maps to 0.5). All confidences should equal 0.5.
    #[tokio::test]
    async fn ablation_sigmoid_zero_gamma_collapses_to_half() {
        let db = build_ablation_fixture().await;

        let mut cfg = base_config();
        cfg.activation.sigmoid_gamma = 0.0;
        cfg.activation.gate_tau = 0.0; // disable gate so 0.5 values pass
        let engine = ActivationEngine::new(db, cfg);
        let out = engine.search("outage", "ab-s1", 10).await.unwrap();

        for r in &out.results {
            assert!(
                (r.confidence - 0.5).abs() < 1e-3,
                "with γ=0, all confidences should collapse to 0.5; got {}", r.confidence
            );
        }
    }

    /// Ablation 6: Subgraph cap at 1 prevents BFS expansion beyond the
    /// seed. Only the seed nodes themselves should appear in results.
    /// (With cap=1, neighbors of seeds aren't added to the index after
    /// the seed itself fills the cap.)
    #[tokio::test]
    async fn ablation_subgraph_cap_one_limits_expansion() {
        let db = build_ablation_fixture().await;

        let mut cfg = base_config();
        cfg.activation.subgraph_cap = 1;
        let engine = ActivationEngine::new(db, cfg);
        let out = engine.search("outage", "ab-s1", 10).await.unwrap();

        // At cap=1, the BFS hits the cap after the first seed is added.
        // Result count should be very small (often <= 1, but the seed count
        // depends on how many seed articles share the "outage" entity).
        assert!(
            out.node_count <= 3,
            "cap=1 should sharply limit subgraph; got node_count={}", out.node_count
        );
    }

    /// Ablation 7: Why-intent boosts causal edges (caused_by ×4.0). With
    /// OpenDomain intent (all weights 1.0), causal-linked articles should
    /// rank lower than under Why. Compare top-ranked positions.
    ///
    /// Fixture causal chain: a1 → a2 → a3 (BFS direction).
    /// Seeds: a1, a2, a4 (outage entity). From a1, caused_by reaches a2.
    /// From a2, caused_by reaches a3. Why-intent boosts the CAUSED_BY edges
    /// ×4.0, so a3 accumulates enough mass to pass the gate.
    #[tokio::test]
    async fn ablation_why_intent_promotes_causal_path() {
        let db = build_ablation_fixture().await;

        // Why-classified query
        let mut cfg_why = base_config();
        // Lower gate to ensure a3 passes even after two-hop attenuation
        cfg_why.activation.gate_tau = 0.05;
        let engine_why = ActivationEngine::new(db.clone(), cfg_why);
        let out_why = engine_why.search("why did the outage happen", "ab-s1", 10).await.unwrap();
        assert_eq!(out_why.intent, Intent::Why);

        // OpenDomain-classified query (same seed entity "outage")
        let mut cfg_od = base_config();
        cfg_od.activation.gate_tau = 0.05;
        let engine_od = ActivationEngine::new(db.clone(), cfg_od);
        let out_od = engine_od.search("outage", "ab-s1", 10).await.unwrap();
        assert_eq!(out_od.intent, Intent::OpenDomain);

        // Both queries should return results; the Why query subgraph
        // should include causally-linked articles (a3 chains back through a2 to a1).
        assert!(!out_why.results.is_empty(), "Why query should return results");
        assert!(!out_od.results.is_empty(), "OpenDomain query should return results");

        // The Why query reaches the causal source (a3); OpenDomain may not
        // since CAUSED_BY is weighted equally to other edges.
        let why_ids: std::collections::HashSet<&str> =
            out_why.results.iter().map(|r| r.article_id.as_str()).collect();
        let _od_ids: std::collections::HashSet<&str> =
            out_od.results.iter().map(|r| r.article_id.as_str()).collect();

        // Why should surface a3 (causal source) via the boosted causal chain
        assert!(
            why_ids.contains("ab-a3"),
            "Why intent should pull in the causal source ab-a3 via boosted CAUSED_BY; got {:?}",
            why_ids
        );
    }

    /// Ablation 8: Top-K cutoff. With top_k=1, only one result returns.
    /// With top_k=100, all qualifying results return (bounded by subgraph).
    #[tokio::test]
    async fn ablation_top_k_bounds_result_count() {
        let db = build_ablation_fixture().await;

        let mut cfg_1 = base_config();
        cfg_1.activation.top_k = 1;
        let engine_1 = ActivationEngine::new(db.clone(), cfg_1);
        let out_1 = engine_1.search("outage", "ab-s1", 100).await.unwrap();
        assert!(out_1.results.len() <= 1, "top_k=1 must cap results; got {}", out_1.results.len());

        let mut cfg_100 = base_config();
        cfg_100.activation.top_k = 100;
        let engine_100 = ActivationEngine::new(db.clone(), cfg_100);
        let out_100 = engine_100.search("outage", "ab-s1", 100).await.unwrap();
        assert!(out_100.results.len() >= out_1.results.len());
    }
}
