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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
    pub name: String,
    pub entity_type: String,
    pub description: Option<String>,
    pub store_id: String,
    pub mention_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    pub id: String,
    pub name: String,
    pub store_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DedupQueueEntry {
    pub id: String,
    pub store_id: String,
    pub incoming_title: String,
    pub incoming_content: String,
    pub incoming_source_type: String,
    pub incoming_source_id: Option<String>,
    pub matched_article_id: String,
    pub content_hash: String,
    pub status: String,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

/// Row returned when querying a MENTIONS edge with entity fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MentionsEdge {
    pub article_id: String,
    pub entity_id: String,
    pub excerpt: String,
    pub confidence: f64,
    pub created_at: String,
}

/// Row returned when querying a RELATED_TO edge between two articles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedToEdge {
    pub from_article_id: String,
    pub to_article_id: String,
    pub shared_entity_count: i64,
    pub strength: f64,
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

    #[test]
    fn test_entity_serde_round_trip() {
        let e = Entity {
            id: "tool:rust".into(),
            name: "Rust".into(),
            entity_type: "tool".into(),
            description: Some("Systems programming language".into()),
            store_id: "s1".into(),
            mention_count: 3,
            created_at: "2026-04-17T00:00:00Z".into(),
            updated_at: "2026-04-17T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        let decoded: Entity = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, "tool:rust");
        assert_eq!(decoded.entity_type, "tool");
        assert_eq!(decoded.description, Some("Systems programming language".into()));
        assert_eq!(decoded.mention_count, 3);
    }

    #[test]
    fn test_tag_serde_round_trip() {
        let t = Tag {
            id: "machine-learning".into(),
            name: "Machine Learning".into(),
            store_id: "s1".into(),
            created_at: "2026-04-17T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&t).unwrap();
        let decoded: Tag = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, "machine-learning");
        assert_eq!(decoded.name, "Machine Learning");
    }

    #[test]
    fn test_dedup_queue_entry_serde_round_trip() {
        let d = DedupQueueEntry {
            id: "dq1".into(),
            store_id: "s1".into(),
            incoming_title: "Duplicate Article".into(),
            incoming_content: "Content here".into(),
            incoming_source_type: "user".into(),
            incoming_source_id: None,
            matched_article_id: "a1".into(),
            content_hash: "abc123".into(),
            status: "pending".into(),
            created_at: "2026-04-17T00:00:00Z".into(),
            resolved_at: None,
        };
        let json = serde_json::to_string(&d).unwrap();
        let decoded: DedupQueueEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.status, "pending");
        assert!(decoded.resolved_at.is_none());
    }
}
