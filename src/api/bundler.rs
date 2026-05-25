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
/// Stub: Task 2 fills in the actual logic.
pub fn pack_to_budget(_results: Vec<K2KResult>, _budget_tokens: u32) -> BundledResponse {
    // P9 Task 2 implementation
    BundledResponse {
        items: vec![],
        total_budget_used: 0,
        items_dropped: 0,
        items_truncated: 0,
    }
}
