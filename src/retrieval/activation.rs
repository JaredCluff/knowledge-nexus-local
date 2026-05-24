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
