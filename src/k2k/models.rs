//! K2K Protocol Data Models
//!
//! Server-side model definitions.  Types that are also used by clients
//! (K2KQueryResponse, K2KResult, ResultProvenance, ClientKey, task types,
//! capability types) are defined in `k2k-common` — the canonical protocol
//! library.  Server-only types (with &'static str, server-specific fields)
//! are defined here.

use serde::{Deserialize, Serialize};

// Re-export PROTOCOL_VERSION from k2k-common for server code that needs it.
pub use k2k::PROTOCOL_VERSION;

// ============================================================================
// Request Types
// ============================================================================

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct K2KQueryRequest {
    pub query: String,
    pub requesting_store: String,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    #[serde(default)]
    pub filters: Option<QueryFilters>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub target_stores: Option<Vec<String>>,
    /// Distributed trace ID for cross-node request correlation (Spec v1.1).
    #[serde(default)]
    pub trace_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct QueryFilters {
    pub paths: Option<Vec<String>>,
    pub file_types: Option<Vec<String>>,
    pub max_file_size_bytes: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct RegisterClientRequest {
    pub client_id: String,
    pub client_name: String,
    pub public_key_pem: String,
    #[serde(default)]
    pub registration_secret: Option<String>,
}

// ============================================================================
// Response Types
// ============================================================================

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub node_id: String,
    pub node_type: &'static str,
    pub capabilities: Vec<String>,
    pub indexed_files: usize,
    pub uptime_seconds: u64,
}

#[derive(Debug, Serialize)]
pub struct NodeInfo {
    pub node_id: String,
    pub node_name: String,
    pub node_type: &'static str,
    pub version: &'static str,
    pub public_key: String,
    pub federation_endpoint: String,
    // Security fields (allowed_paths, blocked_patterns) intentionally omitted —
    // exposing security config on an unauthenticated endpoint enables attackers
    // to craft path-traversal payloads that avoid the blocklist.
}

#[derive(Debug, Serialize)]
pub struct RegisterClientResponse {
    pub status: String,
    pub client_id: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct K2KQueryResponse {
    pub query_id: String,
    pub results: Vec<K2KResult>,
    pub total_results: usize,
    pub stores_queried: Vec<String>,
    pub query_time_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_decision: Option<serde_json::Value>,
    /// Distributed trace ID echoed from the request (Spec v1.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct K2KResult {
    pub article_id: String,
    pub store_id: String,
    pub title: String,
    pub summary: String,
    pub content: String,
    pub confidence: f32,
    pub source_type: String,
    pub tags: Vec<String>,
    pub metadata: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<ResultProvenance>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ResultProvenance {
    pub store_id: String,
    pub store_type: String,
    pub original_rank: usize,
    pub rrf_score: f32,
}

#[derive(Debug, Serialize)]
pub struct K2KError {
    pub error: String,
    pub detail: String,
    pub status_code: u16,
}

// ============================================================================
// JWT Claims
// ============================================================================

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct K2KClaims {
    pub iss: String, // Issuer: "kb:client_id"
    pub iat: i64,    // Issued at
    pub exp: i64,    // Expiration (5 minutes)
    pub transfer_id: String,
    pub client_id: String,
}

// ============================================================================
// Client Registry
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientKey {
    pub client_id: String,
    pub client_name: String,
    pub public_key_pem: String,
    pub registered_at: chrono::DateTime<chrono::Utc>,
}

// ============================================================================
// Helper Functions
// ============================================================================

fn default_top_k() -> usize {
    10
}

impl K2KError {
    pub fn unauthorized(detail: impl Into<String>) -> Self {
        Self {
            error: "Unauthorized".into(),
            detail: detail.into(),
            status_code: 401,
        }
    }

    pub fn forbidden(detail: impl Into<String>) -> Self {
        Self {
            error: "Forbidden".into(),
            detail: detail.into(),
            status_code: 403,
        }
    }

    pub fn internal_error(detail: impl Into<String>) -> Self {
        Self {
            error: "Internal Server Error".into(),
            detail: detail.into(),
            status_code: 500,
        }
    }

    pub fn bad_request(detail: impl Into<String>) -> Self {
        Self {
            error: "Bad Request".into(),
            detail: detail.into(),
            status_code: 400,
        }
    }

    pub fn rate_limited(detail: impl Into<String>, _retry_after_secs: u64) -> Self {
        Self {
            error: "Too Many Requests".into(),
            detail: detail.into(),
            status_code: 429,
        }
    }
}
