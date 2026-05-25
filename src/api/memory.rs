//! /v1/memory/* request/response DTOs (P9).
//!
//! Five endpoints mapped to Lin et al.'s 6-phase memory lifecycle:
//! - observe → Write
//! - recall → Retrieve
//! - reflect → Store (consolidation)
//! - timeline → Retrieve
//! - forget → Forget/Rollback
//!
//! DTOs live here (lib-accessible for integration tests).
//! Axum handlers live in src/k2k/memory_handlers.rs (binary crate context).

use serde::{Deserialize, Serialize};

// Request/response DTOs. Handlers in Tasks 3-7 consume these.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObserveRequest {
    pub text: String,
    #[serde(default)]
    pub modality: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub ts: Option<String>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
    #[serde(default = "default_async")]
    pub r#async: bool,
}

fn default_async() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObserveResponse {
    pub memory_id: String,
    pub accepted: bool,
    #[serde(default)]
    pub reflections_triggered: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallRequest {
    pub query: String,
    #[serde(default)]
    pub token_budget: Option<u32>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub until: Option<String>,
    #[serde(default)]
    pub include_archive: bool,
    #[serde(default)]
    pub federate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecallResponse {
    pub items: Vec<crate::api::bundler::BundledItem>,
    pub total_budget_used: u32,
    pub items_dropped: u32,
    pub items_truncated: u32,
    pub follow_ups: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectRequest {
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default = "default_min_cluster_size")]
    pub min_cluster_size: usize,
}

fn default_min_cluster_size() -> usize { 3 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectResponse {
    pub reflections_generated: Vec<String>,
    pub clusters_skipped: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineQuery {
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub until: Option<String>,
    #[serde(default = "default_timeline_limit")]
    pub limit: usize,
}

fn default_timeline_limit() -> usize { 50 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEvent {
    pub event_id: String,
    pub title: String,
    pub started_at: String,
    pub ended_at: String,
    pub participants: Vec<String>,
    pub evidence_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineResponse {
    pub events: Vec<TimelineEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgetRequest {
    pub memory_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgetResponse {
    pub archived: bool,
    pub audit_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_request_default_async_true() {
        let json = r#"{"text": "Hello"}"#;
        let req: ObserveRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.text, "Hello");
        assert!(req.r#async, "async should default to true");
    }

    #[test]
    fn recall_request_minimal() {
        let json = r#"{"query": "test"}"#;
        let req: RecallRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.query, "test");
        assert!(req.token_budget.is_none());
        assert!(!req.include_archive);
        assert!(!req.federate);
    }

    #[test]
    fn recall_request_full_with_all_fields() {
        let json = r#"{
            "query": "what happened in March",
            "token_budget": 2000,
            "scope": "store_a",
            "since": "2026-03-01T00:00:00Z",
            "until": "2026-03-31T23:59:59Z",
            "include_archive": true,
            "federate": true
        }"#;
        let req: RecallRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.token_budget, Some(2000));
        assert_eq!(req.scope.as_deref(), Some("store_a"));
        assert!(req.include_archive);
        assert!(req.federate);
    }

    #[test]
    fn forget_request_requires_reason() {
        // reason is non-Option<>, so missing → deserialize fails
        let json = r#"{"memory_id": "a1"}"#;
        let res: Result<ForgetRequest, _> = serde_json::from_str(json);
        assert!(res.is_err(), "forget without reason must fail to deserialize");
    }

    #[test]
    fn recall_response_round_trips() {
        use crate::api::bundler::BundledItem;
        let resp = RecallResponse {
            items: vec![
                BundledItem {
                    article_id: "a1".into(),
                    title: "T".into(),
                    summary: "S".into(),
                    confidence: 0.85,
                    truncated: false,
                },
            ],
            total_budget_used: 100,
            items_dropped: 2,
            items_truncated: 0,
            follow_ups: vec!["more like 'T'".into()],
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: RecallResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.items.len(), 1);
        assert_eq!(parsed.total_budget_used, 100);
        assert_eq!(parsed.items_dropped, 2);
        assert_eq!(parsed.follow_ups.len(), 1);
    }
}

