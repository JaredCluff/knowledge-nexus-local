//! /v1/memory/* HTTP handlers (P9).
//!
//! Five endpoints mapped to Lin et al.'s 6-phase memory lifecycle:
//! - observe → Write
//! - recall → Retrieve
//! - reflect → Store (consolidation)
//! - timeline → Retrieve
//! - forget → Forget/Rollback

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
