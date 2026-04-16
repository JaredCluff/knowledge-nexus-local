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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_article_serde_round_trip_with_new_fields() {
        let a = Article {
            id: "a1".into(),
            store_id: "s1".into(),
            title: "T".into(),
            content: "C".into(),
            source_type: "user".into(),
            source_id: "https://example.com/x".into(),
            content_hash: "abc123".into(),
            tags: serde_json::json!(["x"]),
            embedded_at: None,
            created_at: "2026-04-15T00:00:00Z".into(),
            updated_at: "2026-04-15T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&a).unwrap();
        let decoded: Article = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.source_id, "https://example.com/x");
        assert_eq!(decoded.content_hash, "abc123");
    }

    #[test]
    fn test_knowledge_store_serde_has_quantizer_version() {
        let s = KnowledgeStore {
            id: "s1".into(),
            owner_id: "u1".into(),
            store_type: "personal".into(),
            name: "N".into(),
            lancedb_collection: "c".into(),
            quantizer_version: "ivf_pq_v1".into(),
            created_at: "t".into(),
            updated_at: "t".into(),
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("ivf_pq_v1"));
    }
}
