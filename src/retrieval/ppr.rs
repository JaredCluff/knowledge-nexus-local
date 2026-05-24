//! Personalized PageRank over the P5 multi-graph (HippoRAG, arXiv 2405.14831).
//!
//! Given a personalization vector `t` over nodes and a column-stochastic
//! edge-weight matrix `W` (rows = targets, cols = sources; each column sums
//! to 1.0 for non-dangling nodes), iterates:
//!
//!   a^(t+1) = (1 - d) * t + d * W * a^(t)
//!
//! until L1-norm change < tolerance or max_iter is reached. Returns the
//! stationary distribution over nodes as a dense Vec<f32>.
//!
//! Per HippoRAG ablation (arXiv 2405.14831, Table 5): PPR over the KG
//! contributes ~8-10 pts F1 vs naive 1-hop neighborhood expansion.
//! Damping=0.5 is the HippoRAG-tuned default.

use sprs::{CsMat, CsVec};

/// Run Personalized PageRank on a column-stochastic matrix `w`.
///
/// # Arguments
/// - `w`: column-stochastic edge-weight matrix (each column sums to 1).
///   Row index = target node; column index = source node. The (row, col)
///   entry is the transition probability from `col` to `row`.
/// - `personalization`: sparse personalization vector. Will be L1-normalized.
/// - `damping`: restart probability is `(1 - damping)`. HippoRAG default 0.5.
/// - `tolerance`: L1-norm convergence threshold between consecutive iterations.
/// - `max_iter`: hard cap on iterations.
///
/// # Returns
/// The final activation vector (dense f32, length = w.cols()).
pub fn personalized_pagerank(
    w: &CsMat<f32>,
    personalization: &CsVec<f32>,
    damping: f32,
    tolerance: f32,
    max_iter: usize,
) -> Vec<f32> {
    let n = w.cols();
    debug_assert_eq!(personalization.dim(), n,
        "personalization vector dim must match matrix size");

    // L1-normalize the personalization vector
    let t_sum: f32 = personalization.data().iter().sum();
    let t_norm = if t_sum > 0.0 { t_sum } else { 1.0 };

    // Build the personalization in dense form for the (1-d)*t term;
    // sparse iteration would be more efficient but n is bounded by
    // subgraph_cap=10_000 so dense is fine.
    let mut t_dense = vec![0.0_f32; n];
    for (idx, &v) in personalization.indices().iter().zip(personalization.data()) {
        t_dense[*idx] = v / t_norm;
    }

    // Initialize a^(0) from the personalization
    let mut a = t_dense.clone();
    let mut a_next = vec![0.0_f32; n];

    for iter in 0..max_iter {
        // a_next = (1 - d) * t
        for i in 0..n {
            a_next[i] = (1.0 - damping) * t_dense[i];
        }

        // a_next += d * (W * a)
        // W is CsMat in CSR form. Each outer iter gives column j (in CSR
        // this is actually a row, depending on storage order — use the
        // iterator and check). To be safe and portable across sprs versions:
        // - sprs CsMat has `iter()` returning (val, (row, col))
        for (&val, (row, col)) in w.iter() {
            a_next[row] += damping * val * a[col];
        }

        // L1 norm of (a_next - a) for convergence
        let delta: f32 = a.iter().zip(a_next.iter())
            .map(|(p, n)| (p - n).abs())
            .sum();

        std::mem::swap(&mut a, &mut a_next);

        if delta < tolerance {
            tracing::debug!("PPR converged in {} iterations (delta={:.2e})", iter + 1, delta);
            return a;
        }
    }

    tracing::debug!("PPR hit max_iter={} without convergence", max_iter);
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprs::{CsVec, TriMat};

    /// Build a column-stochastic matrix from explicit (target_row, source_col, weight) triples.
    /// The caller must pre-normalize columns to sum to 1.
    fn build_csr(n: usize, triples: &[(usize, usize, f32)]) -> CsMat<f32> {
        let mut tri = TriMat::new((n, n));
        for &(row, col, val) in triples {
            tri.add_triplet(row, col, val);
        }
        tri.to_csr()
    }

    /// On a 3-node line A→B→C with each column probability 1.0, seeding A
    /// should produce monotonically decreasing activation: a(A) > a(B) > a(C).
    /// A's mass is preserved by the (1-d)*t restart term.
    #[test]
    fn ppr_decays_along_simple_chain() {
        // Targets get mass from sources: col=A sends to B; col=B sends to C
        let w = build_csr(3, &[
            (1, 0, 1.0), // B (row 1) receives from A (col 0)
            (2, 1, 1.0), // C (row 2) receives from B (col 1)
        ]);

        let t = CsVec::new(3, vec![0], vec![1.0]); // seed A

        let result = personalized_pagerank(&w, &t, 0.5, 1e-4, 50);

        // A retains the most mass (restart bias)
        assert!(result[0] > result[1], "A should retain more mass than B; got {:?}", result);
        // B has more than C
        assert!(result[1] > result[2], "B should have more mass than C; got {:?}", result);
        // A retains >= 1-d = 0.5 (lower bound from restart)
        assert!(result[0] >= 0.49, "A should retain >=50% from restart; got {}", result[0]);
    }

    /// Star graph with center node 0 and three leaves 1, 2, 3. Mass should
    /// concentrate at center.
    #[test]
    fn ppr_converges_within_max_iter_on_star() {
        let w = build_csr(4, &[
            // Center → each leaf (column 0 sums to 1.0)
            (1, 0, 1.0 / 3.0),
            (2, 0, 1.0 / 3.0),
            (3, 0, 1.0 / 3.0),
            // Each leaf → center
            (0, 1, 1.0),
            (0, 2, 1.0),
            (0, 3, 1.0),
        ]);
        let t = CsVec::new(4, vec![0], vec![1.0]);
        let result = personalized_pagerank(&w, &t, 0.5, 1e-6, 200);

        // After convergence, center has most mass
        let max_idx = result.iter().enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(i, _)| i)
            .unwrap();
        assert_eq!(max_idx, 0, "Center should have most mass; got result {:?}", result);
    }

    /// Two disconnected components A↔B and C↔D. Seeding only A: C and D
    /// should remain at zero (no path).
    #[test]
    fn ppr_isolates_disconnected_components() {
        let w = build_csr(4, &[
            (1, 0, 1.0), (0, 1, 1.0),  // A ↔ B
            (3, 2, 1.0), (2, 3, 1.0),  // C ↔ D
        ]);
        let t = CsVec::new(4, vec![0], vec![1.0]);
        let result = personalized_pagerank(&w, &t, 0.5, 1e-4, 50);

        assert!(result[0] > 0.0, "A should have mass");
        assert!(result[1] > 0.0, "B should have mass");
        assert!(result[2].abs() < 1e-6, "C (disconnected) should be ~0, got {}", result[2]);
        assert!(result[3].abs() < 1e-6, "D (disconnected) should be ~0, got {}", result[3]);
    }
}
