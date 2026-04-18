//! SurrealQL DDL for the 1.0.0 relational layer, including P3 graph edges
//! (`MENTIONS`, `TAGGED`, `RELATED_TO`) and entity/tag tables.
//!
//! The schema is SCHEMAFULL — unknown fields are rejected.

pub const SCHEMA_VERSION: &str = "1.0.0-p3";

/// Returns the full SurrealQL DDL script. Idempotent: every statement uses
/// `IF NOT EXISTS` or `OVERWRITE` so re-running on an initialized DB is a
/// no-op.
pub fn ddl() -> &'static str {
    r#"
-- User
DEFINE TABLE IF NOT EXISTS user SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS username ON user TYPE string ASSERT $value != NONE;
DEFINE FIELD IF NOT EXISTS display_name ON user TYPE string;
DEFINE FIELD IF NOT EXISTS is_owner ON user TYPE bool DEFAULT false;
DEFINE FIELD IF NOT EXISTS settings ON user TYPE object DEFAULT {};
DEFINE FIELD IF NOT EXISTS created_at ON user TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at ON user TYPE string;
DEFINE INDEX IF NOT EXISTS user_username_unique ON user FIELDS username UNIQUE;
DEFINE INDEX IF NOT EXISTS user_is_owner_idx ON user FIELDS is_owner;

-- Knowledge store
DEFINE TABLE IF NOT EXISTS knowledge_store SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS owner_id ON knowledge_store TYPE string;
DEFINE FIELD IF NOT EXISTS store_type ON knowledge_store TYPE string
    ASSERT $value IN ["personal", "family", "shared"];
DEFINE FIELD IF NOT EXISTS name ON knowledge_store TYPE string;
DEFINE FIELD IF NOT EXISTS lancedb_collection ON knowledge_store TYPE string;
DEFINE FIELD IF NOT EXISTS quantizer_version ON knowledge_store TYPE string DEFAULT "ivf_pq_v1";
DEFINE FIELD IF NOT EXISTS created_at ON knowledge_store TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at ON knowledge_store TYPE string;
DEFINE INDEX IF NOT EXISTS knowledge_store_lancedb_collection_unique
    ON knowledge_store FIELDS lancedb_collection UNIQUE;
DEFINE INDEX IF NOT EXISTS knowledge_store_owner_idx
    ON knowledge_store FIELDS owner_id;

-- Article
DEFINE TABLE IF NOT EXISTS article SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS store_id ON article TYPE string;
DEFINE FIELD IF NOT EXISTS title ON article TYPE string;
DEFINE FIELD IF NOT EXISTS content ON article TYPE string;
DEFINE FIELD IF NOT EXISTS source_type ON article TYPE string DEFAULT "user";
DEFINE FIELD IF NOT EXISTS source_id ON article TYPE string DEFAULT "";
DEFINE FIELD IF NOT EXISTS content_hash ON article TYPE string DEFAULT "";
DEFINE FIELD IF NOT EXISTS tags ON article TYPE array DEFAULT [];
DEFINE FIELD IF NOT EXISTS tags.* ON article TYPE string;
DEFINE FIELD IF NOT EXISTS embedded_at ON article TYPE option<string>;
DEFINE FIELD IF NOT EXISTS created_at ON article TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at ON article TYPE string;
DEFINE INDEX IF NOT EXISTS article_store_idx ON article FIELDS store_id;
DEFINE INDEX IF NOT EXISTS article_hash_idx ON article FIELDS store_id, content_hash;

-- FTS: BM25 search over title + content. Analyzer tokenizes on whitespace
-- and applies ASCII + lowercase folding. SurrealDB BM25 is the FTS5 analog.
DEFINE ANALYZER IF NOT EXISTS article_ascii
    TOKENIZERS class FILTERS ascii, lowercase;
DEFINE INDEX IF NOT EXISTS article_title_search
    ON article FIELDS title SEARCH ANALYZER article_ascii BM25 HIGHLIGHTS;
DEFINE INDEX IF NOT EXISTS article_content_search
    ON article FIELDS content SEARCH ANALYZER article_ascii BM25 HIGHLIGHTS;

-- Conversation
DEFINE TABLE IF NOT EXISTS conversation SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS user_id ON conversation TYPE string;
DEFINE FIELD IF NOT EXISTS title ON conversation TYPE string DEFAULT "New Conversation";
DEFINE FIELD IF NOT EXISTS message_count ON conversation TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS created_at ON conversation TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at ON conversation TYPE string;
DEFINE INDEX IF NOT EXISTS conversation_user_idx ON conversation FIELDS user_id;

-- Message
DEFINE TABLE IF NOT EXISTS message SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS conversation_id ON message TYPE string;
DEFINE FIELD IF NOT EXISTS role ON message TYPE string
    ASSERT $value IN ["user", "assistant", "system"];
DEFINE FIELD IF NOT EXISTS content ON message TYPE string;
DEFINE FIELD IF NOT EXISTS metadata ON message TYPE object DEFAULT {};
DEFINE FIELD IF NOT EXISTS created_at ON message TYPE string;
DEFINE INDEX IF NOT EXISTS message_conversation_idx ON message FIELDS conversation_id;

-- K2K client
DEFINE TABLE IF NOT EXISTS k2k_client SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS public_key_pem ON k2k_client TYPE string;
DEFINE FIELD IF NOT EXISTS client_name ON k2k_client TYPE string DEFAULT "";
DEFINE FIELD IF NOT EXISTS registered_at ON k2k_client TYPE string;
DEFINE FIELD IF NOT EXISTS status ON k2k_client TYPE string DEFAULT "approved";
DEFINE INDEX IF NOT EXISTS k2k_client_status_idx ON k2k_client FIELDS status;

-- Federation agreement
DEFINE TABLE IF NOT EXISTS federation_agreement SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS local_store_id ON federation_agreement TYPE string;
DEFINE FIELD IF NOT EXISTS remote_node_id ON federation_agreement TYPE string;
DEFINE FIELD IF NOT EXISTS remote_endpoint ON federation_agreement TYPE string;
DEFINE FIELD IF NOT EXISTS access_type ON federation_agreement TYPE string
    ASSERT $value IN ["read", "write", "readwrite"];
DEFINE FIELD IF NOT EXISTS created_at ON federation_agreement TYPE string;
DEFINE INDEX IF NOT EXISTS federation_agreement_store_idx
    ON federation_agreement FIELDS local_store_id;

-- Discovered node
DEFINE TABLE IF NOT EXISTS discovered_node SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS host ON discovered_node TYPE string;
DEFINE FIELD IF NOT EXISTS port ON discovered_node TYPE int
    ASSERT $value >= 1 AND $value <= 65535;
DEFINE FIELD IF NOT EXISTS endpoint ON discovered_node TYPE string;
DEFINE FIELD IF NOT EXISTS capabilities ON discovered_node TYPE array DEFAULT [];
DEFINE FIELD IF NOT EXISTS capabilities.* ON discovered_node TYPE string;
DEFINE FIELD IF NOT EXISTS last_seen ON discovered_node TYPE string;
DEFINE FIELD IF NOT EXISTS healthy ON discovered_node TYPE bool DEFAULT true;

-- Connector config
DEFINE TABLE IF NOT EXISTS connector_config SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS connector_type ON connector_config TYPE string;
DEFINE FIELD IF NOT EXISTS name ON connector_config TYPE string;
DEFINE FIELD IF NOT EXISTS config ON connector_config TYPE object DEFAULT {};
DEFINE FIELD IF NOT EXISTS store_id ON connector_config TYPE string;
DEFINE FIELD IF NOT EXISTS created_at ON connector_config TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at ON connector_config TYPE string;
DEFINE INDEX IF NOT EXISTS connector_config_store_idx
    ON connector_config FIELDS store_id;

-- Schema-version tracking table (used by `src/store/migrations.rs`).
DEFINE TABLE IF NOT EXISTS _schema_version SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS version ON _schema_version TYPE string;
DEFINE FIELD IF NOT EXISTS applied_at ON _schema_version TYPE string;

-- Entity (P3)
DEFINE TABLE IF NOT EXISTS entity SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS name ON entity TYPE string;
DEFINE FIELD IF NOT EXISTS entity_type ON entity TYPE string
    ASSERT $value IN ["topic", "person", "project", "tool", "concept", "reference"];
DEFINE FIELD IF NOT EXISTS description ON entity TYPE option<string>;
DEFINE FIELD IF NOT EXISTS store_id ON entity TYPE string;
DEFINE FIELD IF NOT EXISTS mention_count ON entity TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS created_at ON entity TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at ON entity TYPE string;
DEFINE INDEX IF NOT EXISTS entity_store_name_type_unique
    ON entity FIELDS store_id, name, entity_type UNIQUE;
DEFINE INDEX IF NOT EXISTS entity_store_idx ON entity FIELDS store_id;

-- Tag (P3)
DEFINE TABLE IF NOT EXISTS tag SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS name ON tag TYPE string;
DEFINE FIELD IF NOT EXISTS store_id ON tag TYPE string;
DEFINE FIELD IF NOT EXISTS created_at ON tag TYPE string;
DEFINE INDEX IF NOT EXISTS tag_store_name_unique ON tag FIELDS store_id, name UNIQUE;

-- Dedup queue (P3)
DEFINE TABLE IF NOT EXISTS dedup_queue SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS store_id ON dedup_queue TYPE string;
DEFINE FIELD IF NOT EXISTS incoming_title ON dedup_queue TYPE string;
DEFINE FIELD IF NOT EXISTS incoming_content ON dedup_queue TYPE string;
DEFINE FIELD IF NOT EXISTS incoming_source_type ON dedup_queue TYPE string;
DEFINE FIELD IF NOT EXISTS incoming_source_id ON dedup_queue TYPE option<string>;
DEFINE FIELD IF NOT EXISTS matched_article_id ON dedup_queue TYPE string;
DEFINE FIELD IF NOT EXISTS content_hash ON dedup_queue TYPE string;
DEFINE FIELD IF NOT EXISTS status ON dedup_queue TYPE string DEFAULT "pending"
    ASSERT $value IN ["pending", "rejected", "merged"];
DEFINE FIELD IF NOT EXISTS created_at ON dedup_queue TYPE string;
DEFINE FIELD IF NOT EXISTS resolved_at ON dedup_queue TYPE option<string>;
DEFINE INDEX IF NOT EXISTS dedup_queue_status_idx ON dedup_queue FIELDS status;
DEFINE INDEX IF NOT EXISTS dedup_queue_store_idx ON dedup_queue FIELDS store_id;
DEFINE INDEX IF NOT EXISTS dedup_queue_hash_idx ON dedup_queue FIELDS store_id, content_hash;

-- TAGGED edge table (P3): article -> tag
DEFINE TABLE IF NOT EXISTS tagged SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS in ON tagged TYPE record<article>;
DEFINE FIELD IF NOT EXISTS out ON tagged TYPE record<tag>;
DEFINE FIELD IF NOT EXISTS created_at ON tagged TYPE string;
DEFINE INDEX IF NOT EXISTS tagged_unique ON tagged FIELDS in, out UNIQUE;

-- MENTIONS edge table (P3): article -> entity
DEFINE TABLE IF NOT EXISTS mentions SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS in ON mentions TYPE record<article>;
DEFINE FIELD IF NOT EXISTS out ON mentions TYPE record<entity>;
DEFINE FIELD IF NOT EXISTS excerpt ON mentions TYPE string;
DEFINE FIELD IF NOT EXISTS confidence ON mentions TYPE float DEFAULT 0.0;
DEFINE FIELD IF NOT EXISTS created_at ON mentions TYPE string;
DEFINE INDEX IF NOT EXISTS mentions_unique ON mentions FIELDS in, out UNIQUE;

-- RELATED_TO edge table (P3): article -> article
DEFINE TABLE IF NOT EXISTS related_to SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS in ON related_to TYPE record<article>;
DEFINE FIELD IF NOT EXISTS out ON related_to TYPE record<article>;
DEFINE FIELD IF NOT EXISTS shared_entity_count ON related_to TYPE int DEFAULT 0;
DEFINE FIELD IF NOT EXISTS strength ON related_to TYPE float DEFAULT 0.0;
DEFINE FIELD IF NOT EXISTS created_at ON related_to TYPE string;
DEFINE FIELD IF NOT EXISTS updated_at ON related_to TYPE string;
DEFINE INDEX IF NOT EXISTS related_to_unique ON related_to FIELDS in, out UNIQUE;
"#
}
