//! SurrealQL DDL for the 1.0.0 relational layer.
//!
//! The schema is SCHEMAFULL — unknown fields are rejected. Graph edges
//! (`MENTIONS`, `TAGGED`, etc.) and entity/tag tables are added in P3 and
//! are intentionally absent from this file.

pub const SCHEMA_VERSION: &str = "1.0.0-p1";

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
"#
}
