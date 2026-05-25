//! Token-budget bundler for /v1/memory/recall.
//!
//! Packs ranked results into a response that fits within the requested
//! token budget. Char-based heuristic: ~4 chars per token. Drops or
//! truncates items as needed.

use serde::{Deserialize, Serialize};

use crate::k2k::models::K2KResult;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundledItem {
    pub article_id: String,
    pub title: String,
    pub summary: String,
    pub confidence: f32,
    /// True if the summary was truncated to fit the budget.
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundledResponse {
    pub items: Vec<BundledItem>,
    pub total_budget_used: u32,
    pub items_dropped: u32,
    pub items_truncated: u32,
}

/// Pack ranked results into a response fitting the budget.
///
/// Iterates results in rank order; estimates each item's token cost via
/// `(title + summary).chars().count() / 4`; adds items until adding the
/// next would exceed budget.
///
/// If even the first item exceeds budget, its summary is truncated to fit.
/// All subsequent items are dropped.
///
/// Returns counters for budget_used, items_dropped, items_truncated so
/// the caller can surface telemetry.
pub fn pack_to_budget(results: Vec<K2KResult>, budget_tokens: u32) -> BundledResponse {
    if results.is_empty() || budget_tokens == 0 {
        return BundledResponse {
            items: vec![],
            total_budget_used: 0,
            items_dropped: results.len() as u32,
            items_truncated: 0,
        };
    }

    let budget_chars = budget_tokens.saturating_mul(4) as usize;
    let mut items = Vec::new();
    let mut used_chars: usize = 0;
    let mut items_dropped: u32 = 0;
    let mut items_truncated: u32 = 0;

    for r in results {
        let item_chars = r.title.chars().count() + r.summary.chars().count();

        // Headroom for this item
        let remaining = budget_chars.saturating_sub(used_chars);

        if remaining == 0 {
            items_dropped += 1;
            continue;
        }

        if item_chars <= remaining {
            // Fits in full
            items.push(BundledItem {
                article_id: r.article_id,
                title: r.title,
                summary: r.summary,
                confidence: r.confidence,
                truncated: false,
            });
            used_chars += item_chars;
        } else if items.is_empty() {
            // First item exceeds budget; truncate its summary so SOMETHING is returned.
            let title_chars = r.title.chars().count();
            let remaining_for_summary = remaining.saturating_sub(title_chars);
            let truncated_summary = if remaining_for_summary == 0 {
                String::new()
            } else {
                // Truncate at char boundary
                let mut s = String::with_capacity(remaining_for_summary);
                for c in r.summary.chars().take(remaining_for_summary) {
                    s.push(c);
                }
                s
            };

            items.push(BundledItem {
                article_id: r.article_id,
                title: r.title,
                summary: truncated_summary,
                confidence: r.confidence,
                truncated: true,
            });
            items_truncated += 1;
            used_chars = budget_chars;
        } else {
            // Subsequent items that exceed remaining budget are dropped.
            items_dropped += 1;
        }
    }

    BundledResponse {
        items,
        total_budget_used: ((used_chars + 3) / 4) as u32, // round up to tokens
        items_dropped,
        items_truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::k2k::models::{K2KResult, ResultProvenance};

    fn mk(id: &str, title: &str, summary: &str, conf: f32) -> K2KResult {
        K2KResult {
            article_id: id.into(),
            store_id: "s1".into(),
            title: title.into(),
            summary: summary.into(),
            content: String::new(),
            confidence: conf,
            source_type: "user".into(),
            tags: vec![],
            metadata: serde_json::json!({}),
            provenance: Some(ResultProvenance {
                store_id: "s1".into(),
                store_type: "test".into(),
                original_rank: 0,
                rrf_score: 0.0,
            }),
        }
    }

    #[test]
    fn empty_results_returns_empty_response() {
        let resp = pack_to_budget(vec![], 1000);
        assert_eq!(resp.items.len(), 0);
        assert_eq!(resp.items_dropped, 0);
        assert_eq!(resp.total_budget_used, 0);
    }

    #[test]
    fn single_item_fits_within_budget() {
        let results = vec![mk("a1", "Title", "Brief summary", 0.8)];
        let resp = pack_to_budget(results, 1000);
        assert_eq!(resp.items.len(), 1);
        assert_eq!(resp.items[0].article_id, "a1");
        assert!(!resp.items[0].truncated);
        assert!(resp.total_budget_used > 0);
    }

    #[test]
    fn first_item_exceeds_budget_gets_truncated() {
        let huge_summary = "x".repeat(10_000);
        let results = vec![mk("big", "Big Article", &huge_summary, 0.9)];
        // Budget: 50 tokens = 200 chars
        let resp = pack_to_budget(results, 50);
        assert_eq!(resp.items.len(), 1, "first item must be truncated, not dropped");
        assert!(resp.items[0].truncated);
        assert_eq!(resp.items_truncated, 1);
        // Title (11 chars) + truncated summary must fit budget
        let total_chars = resp.items[0].title.chars().count() + resp.items[0].summary.chars().count();
        assert!(total_chars <= 200, "total chars {} should be ≤ 200", total_chars);
    }

    #[test]
    fn total_exceeds_budget_drops_tail_items() {
        // 3 items each ~30 chars. Budget 50 chars = 12.5 tokens. Round to 13 tokens budget.
        // budget_chars = 52. First item fits (~30 chars), second item makes total 60, exceeds, dropped.
        let results = vec![
            mk("a", "Item 1", "Twenty-eight char summary here.", 0.9),
            mk("b", "Item 2", "Another summary same length yes", 0.8),
            mk("c", "Item 3", "Third summary also similar size", 0.7),
        ];
        let resp = pack_to_budget(results, 13);
        assert_eq!(resp.items.len(), 1, "only first item should fit; got {}", resp.items.len());
        assert_eq!(resp.items_dropped, 2);
        assert_eq!(resp.items[0].article_id, "a");
        assert!(!resp.items[0].truncated);
    }
}
