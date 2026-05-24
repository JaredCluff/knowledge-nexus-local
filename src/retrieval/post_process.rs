//! SYNAPSE post-PPR processing (arXiv 2601.02744): lateral inhibition,
//! sigmoid normalization, confidence gating, top-K selection.
//!
//! Applied AFTER PPR converges. Provides competition between high-activation
//! nodes (which PPR alone doesn't), squashes values to [0, 1], drops
//! low-confidence noise, and produces the final ranking.
//!
//! All constants are SYNAPSE published defaults:
//! - inhibition β=0.15, top-M=7 competitors
//! - sigmoid γ=5.0
//! - gate τ=0.12

/// Apply lateral inhibition over the top-M competitors of node i:
/// `û_i = max(0, u_i - β·Σ_{k∈T_M}(u_k - u_i)·𝕀[u_k > u_i])`
///
/// Prevents hub-node domination; results diversify across the top of the
/// distribution while preserving order between high vs low activation.
pub fn lateral_inhibition(values: &[f32], beta: f32, m: usize) -> Vec<f32> {
    if values.len() <= 1 || beta <= 0.0 {
        return values.to_vec();
    }

    // Find top-M competitor values (we only care about values, not indices)
    let mut sorted: Vec<f32> = values.to_vec();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let top_m: Vec<f32> = sorted.into_iter().take(m).collect();

    let mut out = vec![0.0_f32; values.len()];
    for (i, &u_i) in values.iter().enumerate() {
        let inhibition: f32 = top_m.iter()
            .filter(|u_k| **u_k > u_i)
            .map(|u_k| u_k - u_i)
            .sum();
        out[i] = (u_i - beta * inhibition).max(0.0);
    }
    out
}

/// Sigmoid normalization with steepness γ: maps each value through
/// `1 / (1 + exp(-γ*u))` to squash into (0, 1).
pub fn sigmoid_normalize(values: &[f32], gamma: f32) -> Vec<f32> {
    values.iter()
        .map(|&u| 1.0 / (1.0 + (-gamma * u).exp()))
        .collect()
}

/// Drop entries below `tau`; return (original_index, value) of survivors
/// sorted descending by value.
pub fn confidence_gate(values: &[f32], tau: f32) -> Vec<(usize, f32)> {
    let mut survivors: Vec<(usize, f32)> = values.iter()
        .enumerate()
        .filter(|(_, &v)| v >= tau)
        .map(|(i, &v)| (i, v))
        .collect();
    survivors.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    survivors
}

/// Full post-processing pipeline: inhibition → sigmoid → gate → top-K.
/// Returns (original_index, normalized_score) tuples sorted desc.
pub fn post_process(
    ppr_values: &[f32],
    beta: f32,
    m: usize,
    gamma: f32,
    tau: f32,
    top_k: usize,
) -> Vec<(usize, f32)> {
    let inhibited = lateral_inhibition(ppr_values, beta, m);
    let normalized = sigmoid_normalize(&inhibited, gamma);
    let mut gated = confidence_gate(&normalized, tau);
    gated.truncate(top_k);
    gated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lateral_inhibition_suppresses_below_top() {
        let values = [1.0, 0.9, 0.5, 0.1];
        let out = lateral_inhibition(&values, 0.15, 7);
        // Top value unchanged (no higher competitors)
        assert!((out[0] - 1.0).abs() < 1e-6, "top value should be ~1.0, got {}", out[0]);
        // Others reduced by inhibition
        assert!(out[1] < 0.9, "0.9 should be reduced; got {}", out[1]);
        assert!(out[3] < 0.1, "0.1 should be heavily reduced; got {}", out[3]);
    }

    #[test]
    fn lateral_inhibition_zero_beta_is_identity() {
        let values = [1.0, 0.5, 0.1];
        let out = lateral_inhibition(&values, 0.0, 7);
        assert_eq!(out, values);
    }

    #[test]
    fn lateral_inhibition_clamps_to_zero() {
        // Strong competitor; weaker value should clamp to 0, not go negative
        let values = [10.0, 0.0];
        let out = lateral_inhibition(&values, 0.5, 7);
        assert!(out[1] >= 0.0, "result must be non-negative, got {}", out[1]);
    }

    #[test]
    fn sigmoid_squashes_to_unit_interval() {
        let values = [-10.0, 0.0, 10.0];
        let out = sigmoid_normalize(&values, 1.0);
        assert!(out[0] < 0.001, "very negative input should give ~0");
        assert!((out[1] - 0.5).abs() < 1e-6, "0 input should give 0.5");
        assert!(out[2] > 0.999, "very positive input should give ~1");
    }

    #[test]
    fn confidence_gate_drops_below_tau_and_sorts() {
        let values = [0.5, 0.2, 0.8, 0.05];
        let survivors = confidence_gate(&values, 0.3);
        let ids: Vec<usize> = survivors.iter().map(|(i, _)| *i).collect();
        // Sorted desc: 0.8 (idx 2), 0.5 (idx 0); 0.2 and 0.05 dropped
        assert_eq!(ids, vec![2, 0]);
    }

    #[test]
    fn post_process_full_pipeline_preserves_top_ranking() {
        let values = [3.0, 2.0, 1.0, 0.5, 0.1];
        let out = post_process(&values, 0.15, 7, 5.0, 0.12, 3);

        // Bounded by top_k = 3
        assert!(out.len() <= 3);
        // Highest input should still rank first
        assert_eq!(out[0].0, 0, "highest input should win; got {:?}", out);
        // All outputs in [0, 1]
        for &(_, v) in &out {
            assert!((0.0..=1.0).contains(&v), "value out of range: {}", v);
        }
        // Descending order
        if out.len() >= 2 {
            assert!(out[0].1 >= out[1].1, "results must be sorted desc");
        }
    }
}
