//! Data models for the SurrealDB-backed store.
//!
//! These structs mirror the SQLite models in `db::models` but are the
//! canonical types used by the `Store` trait and all P1+ code paths.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub is_owner: bool,
    pub settings: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeStore {
    pub id: String,
    pub owner_id: String,
    pub store_type: String, // "personal", "family", "shared"
    pub name: String,
    pub lancedb_collection: String,
    /// Which `VectorQuantizer` impl this store uses. Populated by P2 dispatch;
    /// migrated rows default to "ivf_pq_v1" via the schema default.
    pub quantizer_version: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Article {
    pub id: String,
    pub store_id: String,
    pub title: String,
    pub content: String,
    pub source_type: String,
    /// Stable identifier from the source connector (URL, RSS GUID, etc.).
    /// Empty string for articles that predate 1.0.0; connectors populate it
    /// going forward (wired in P3).
    pub source_id: String,
    /// SHA-256 of normalized content. Used by P3 dedup; backfilled during
    /// migration.
    pub content_hash: String,
    /// JSON array of tag strings — normalized into a `tag` table + `TAGGED`
    /// edges in P3. Kept as JSON on the article through P1.
    pub tags: serde_json::Value,
    pub embedded_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub user_id: String,
    pub title: String,
    pub message_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub role: String,
    pub content: String,
    pub metadata: serde_json::Value,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct K2KClient {
    pub client_id: String,
    pub public_key_pem: String,
    pub client_name: String,
    pub registered_at: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationAgreement {
    pub id: String,
    pub local_store_id: String,
    pub remote_node_id: String,
    pub remote_endpoint: String,
    pub access_type: String, // "read", "write", "readwrite"
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredNode {
    pub node_id: String,
    pub host: String,
    pub port: u16,
    pub endpoint: String,
    pub capabilities: serde_json::Value,
    pub last_seen: String,
    pub healthy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct ConnectorConfig {
    pub id: String,
    pub connector_type: String,
    pub name: String,
    pub config: serde_json::Value,
    pub store_id: String,
    pub created_at: String,
    pub updated_at: String,
}
