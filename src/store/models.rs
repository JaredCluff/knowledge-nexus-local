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
    /// edges in P3. After P3 migration, this field is removed from the DB schema.
    /// `#[serde(default)]` ensures deserialization still works when the field is absent.
    #[serde(default)]
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

/// How an edge was derived. Stored as a lowercase string in SurrealDB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionMethod {
    /// Deterministic / rule-based extraction (e.g. timestamps for temporal,
    /// entity-overlap for ENTITY_OVERLAP). Cheap and reproducible.
    Heuristic,
    /// LLM-driven extraction (currently only CAUSED_BY in P5).
    Llm,
    /// User explicitly asserted this edge (e.g. markdown citation).
    UserAsserted,
    /// Derived from another signal (e.g. SEMANTICALLY_RELATED via LanceDB ANN).
    Derived,
}

/// Row returned when querying an ENTITY_OVERLAP edge (renamed from `RelatedToEdge`
/// in P5). Same Jaccard-on-shared-entities semantics as P3's `RELATED_TO`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct EntityOverlapEdge {
    pub from_article_id: String,
    pub to_article_id: String,
    pub shared_entity_count: i64,
    pub strength: f64,
    pub confidence: f64,
    pub extraction_method: ExtractionMethod,
    pub created_at: String,
    pub updated_at: String,
}

/// Row returned when querying a SEMANTICALLY_RELATED edge. Built from
/// LanceDB ANN: `cos(embedding_i, embedding_j) > θ_sim` (default 0.85).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct SemanticallyRelatedEdge {
    pub from_article_id: String,
    pub to_article_id: String,
    pub similarity: f64,
    pub confidence: f64,
    pub extraction_method: ExtractionMethod,
    pub created_at: String,
}

/// Row returned when querying a PRECEDES edge. Built deterministically from
/// `article.created_at` ordering within an entity-overlap cluster.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct PrecedesEdge {
    pub from_article_id: String,
    pub to_article_id: String,
    pub confidence: f64,
    pub extraction_method: ExtractionMethod,
    pub created_at: String,
}

/// Row returned when querying a CAUSED_BY edge. LLM-extracted; `rationale` is
/// the LLM's verbatim justification for the causal claim (stored for audit).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct CausedByEdge {
    pub from_article_id: String,
    pub to_article_id: String,
    pub confidence: f64,
    pub rationale: Option<String>,
    pub extraction_method: ExtractionMethod,
    pub created_at: String,
}

/// Row returned when querying a REFERENCES_EDGE. Built from explicit markdown
/// links `[anchor](target_article_id)` inside article content.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct ReferencesEdgeRow {
    pub from_article_id: String,
    pub to_article_id: String,
    pub confidence: f64,
    pub extraction_method: ExtractionMethod,
    pub anchor_text: Option<String>,
    pub created_at: String,
}

/// Event: a first-class memory node representing a coherent time-bounded
/// experience (a conversation, a trip, an incident). P7.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub store_id: String,
    pub title: String,
    pub summary: String,
    pub started_at: String,
    pub ended_at: String,
    /// JSON array of participant names/ids.
    pub participants: serde_json::Value,
    /// "conversation" | "manual" | "derived"
    pub source_type: String,
    pub confidence: f64,
    pub extraction_method: ExtractionMethod,
    pub created_at: String,
    pub updated_at: String,
}

/// CONTAINS_EVIDENCE edge: event → article (evidence the event happened).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainsEvidenceEdge {
    pub from_event_id: String,
    pub to_article_id: String,
    pub confidence: f64,
    pub created_at: String,
}

/// MOTIVATES edge: event → event (one event motivated another).
/// CompassMem relation taxonomy. LLM-extracted only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MotivatesEdge {
    pub from_event_id: String,
    pub to_event_id: String,
    pub confidence: f64,
    pub rationale: Option<String>,
    pub extraction_method: ExtractionMethod,
    pub created_at: String,
}

/// PART_OF edge: child event → parent event (hierarchical composition).
/// CompassMem relation taxonomy. LLM-extracted only.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartOfEdge {
    pub from_event_id: String,
    pub to_parent_event_id: String,
    pub confidence: f64,
    pub extraction_method: ExtractionMethod,
    pub created_at: String,
}

/// Per-edge-type row counts for a store. Returned by `Store::count_edges_by_type`.
/// Used by `graph stats` to surface multi-graph coverage at a glance.
#[derive(Debug, Clone, Default)]
pub struct EdgeCounts {
    pub entity_overlap: i64,
    pub semantically_related: i64,
    pub precedes: i64,
    pub caused_by: i64,
    pub references_edge: i64,
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

    #[test]
    fn test_extraction_method_serde() {
        let m = ExtractionMethod::Heuristic;
        let s = serde_json::to_string(&m).unwrap();
        assert_eq!(s, "\"heuristic\"");
        let back: ExtractionMethod = serde_json::from_str("\"llm\"").unwrap();
        assert_eq!(back, ExtractionMethod::Llm);
    }

    #[test]
    fn test_entity_overlap_edge_serde() {
        let e = EntityOverlapEdge {
            from_article_id: "a1".into(),
            to_article_id: "a2".into(),
            shared_entity_count: 3,
            strength: 0.42,
            confidence: 0.42,
            extraction_method: ExtractionMethod::Heuristic,
            created_at: "2026-05-23T00:00:00Z".into(),
            updated_at: "2026-05-23T00:00:00Z".into(),
        };
        let j = serde_json::to_string(&e).unwrap();
        let d: EntityOverlapEdge = serde_json::from_str(&j).unwrap();
        assert_eq!(d.strength, 0.42);
        assert_eq!(d.extraction_method, ExtractionMethod::Heuristic);
    }

    #[test]
    fn test_semantically_related_edge_serde() {
        let e = SemanticallyRelatedEdge {
            from_article_id: "a1".into(),
            to_article_id: "a2".into(),
            similarity: 0.93,
            confidence: 0.93,
            extraction_method: ExtractionMethod::Derived,
            created_at: "2026-05-23T00:00:00Z".into(),
        };
        let j = serde_json::to_string(&e).unwrap();
        let d: SemanticallyRelatedEdge = serde_json::from_str(&j).unwrap();
        assert_eq!(d.similarity, 0.93);
    }

    #[test]
    fn test_precedes_edge_serde() {
        let e = PrecedesEdge {
            from_article_id: "a1".into(),
            to_article_id: "a2".into(),
            confidence: 1.0,
            extraction_method: ExtractionMethod::Heuristic,
            created_at: "2026-05-23T00:00:00Z".into(),
        };
        let j = serde_json::to_string(&e).unwrap();
        let d: PrecedesEdge = serde_json::from_str(&j).unwrap();
        assert_eq!(d.confidence, 1.0);
    }

    #[test]
    fn test_caused_by_edge_serde() {
        let e = CausedByEdge {
            from_article_id: "a1".into(),
            to_article_id: "a2".into(),
            confidence: 0.78,
            rationale: Some("explicit causal language in source".into()),
            extraction_method: ExtractionMethod::Llm,
            created_at: "2026-05-23T00:00:00Z".into(),
        };
        let j = serde_json::to_string(&e).unwrap();
        let d: CausedByEdge = serde_json::from_str(&j).unwrap();
        assert_eq!(d.confidence, 0.78);
        assert_eq!(d.rationale.as_deref(), Some("explicit causal language in source"));
    }

    #[test]
    fn test_references_edge_row_serde() {
        let e = ReferencesEdgeRow {
            from_article_id: "a1".into(),
            to_article_id: "a2".into(),
            confidence: 1.0,
            extraction_method: ExtractionMethod::UserAsserted,
            anchor_text: Some("see [the deploy retro](a2)".into()),
            created_at: "2026-05-23T00:00:00Z".into(),
        };
        let j = serde_json::to_string(&e).unwrap();
        let d: ReferencesEdgeRow = serde_json::from_str(&j).unwrap();
        assert_eq!(d.anchor_text.as_deref(), Some("see [the deploy retro](a2)"));
    }

    #[test]
    fn test_event_serde_round_trip() {
        let e = Event {
            id: "e1".into(),
            store_id: "s1".into(),
            title: "AZ trip March 2026".into(),
            summary: "Family trip to Arizona mountains".into(),
            started_at: "2026-03-15T00:00:00Z".into(),
            ended_at: "2026-03-20T00:00:00Z".into(),
            participants: serde_json::json!(["alice", "bob"]),
            source_type: "conversation".into(),
            confidence: 0.85,
            extraction_method: ExtractionMethod::Llm,
            created_at: "2026-05-24T00:00:00Z".into(),
            updated_at: "2026-05-24T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&e).unwrap();
        let d: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(d.title, "AZ trip March 2026");
        assert_eq!(d.extraction_method, ExtractionMethod::Llm);
    }

    #[test]
    fn test_contains_evidence_edge_serde_round_trip() {
        let edge = ContainsEvidenceEdge {
            from_event_id: "e1".into(),
            to_article_id: "a1".into(),
            confidence: 0.9,
            created_at: "2026-05-24T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&edge).unwrap();
        let d: ContainsEvidenceEdge = serde_json::from_str(&json).unwrap();
        assert_eq!(d.from_event_id, "e1");
    }

    #[test]
    fn test_motivates_edge_serde_round_trip() {
        let edge = MotivatesEdge {
            from_event_id: "e1".into(),
            to_event_id: "e2".into(),
            confidence: 0.75,
            rationale: Some("user pursued e2 because of e1 outcome".into()),
            extraction_method: ExtractionMethod::Llm,
            created_at: "2026-05-24T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&edge).unwrap();
        let d: MotivatesEdge = serde_json::from_str(&json).unwrap();
        assert_eq!(d.confidence, 0.75);
        assert_eq!(d.rationale.as_deref(), Some("user pursued e2 because of e1 outcome"));
    }

    #[test]
    fn test_part_of_edge_serde_round_trip() {
        let edge = PartOfEdge {
            from_event_id: "sub_e1".into(),
            to_parent_event_id: "parent_e1".into(),
            confidence: 0.95,
            extraction_method: ExtractionMethod::Llm,
            created_at: "2026-05-24T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&edge).unwrap();
        let d: PartOfEdge = serde_json::from_str(&json).unwrap();
        assert_eq!(d.from_event_id, "sub_e1");
    }
}
