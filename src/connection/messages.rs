//! Protocol messages for Agent Hub communication.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Message types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    AuthRequest,
    AuthResponse,
    Heartbeat,
    HeartbeatAck,
    QueryRequest,
    QueryResponse,
    ChatMessage,
    ChatResponse,
    Error,
    Disconnect,
}

/// Message envelope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEnvelope {
    #[serde(rename = "type")]
    pub msg_type: MessageType,
    pub payload: serde_json::Value,
    pub timestamp: String,
}

/// Device information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    pub platform_version: String,
    pub agent_version: String,
}

/// Authentication request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthRequest {
    pub protocol_version: String,
    pub auth_method: String,
    pub token: String,
    pub device: DeviceInfo,
    pub capabilities: Vec<String>,
    pub allowed_paths: Vec<String>,
    pub blocked_patterns: Vec<String>,
}

/// Authentication response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub success: bool,
    pub session_id: Option<String>,
    pub user_id: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub heartbeat_interval_seconds: u32,
    pub server_time: String,
}

/// Heartbeat message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Heartbeat {
    pub session_id: String,
    pub status: String,
    pub metrics: Option<AgentMetrics>,
}

/// Agent metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetrics {
    pub queries_processed: u64,
    pub avg_query_time_ms: f64,
    pub indexed_files: u64,
    pub index_size_bytes: u64,
}

/// Heartbeat acknowledgment
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct HeartbeatAck {
    pub server_time: String,
}

/// Query request from hub
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRequest {
    pub query_id: String,
    pub query: String,
    pub query_type: String,
    pub filters: Option<QueryFilters>,
    pub options: Option<QueryOptions>,
    pub timeout_ms: u32,
}

/// Query filters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryFilters {
    pub paths: Option<Vec<String>>,
    pub file_types: Option<Vec<String>>,
    pub max_results: Option<usize>,
    pub max_file_size_bytes: Option<u64>,
}

/// Query options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryOptions {
    pub include_content: Option<bool>,
    pub include_metadata: Option<bool>,
    pub highlight_matches: Option<bool>,
}

/// Query response to hub
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResponse {
    pub query_id: String,
    pub success: bool,
    pub results: Vec<FileResult>,
    pub total_count: usize,
    pub returned_count: usize,
    pub execution_time_ms: u64,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

/// File result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileResult {
    pub id: String,
    pub path: String,
    pub filename: String,
    pub content: Option<String>,
    pub content_type: String,
    pub score: f32,
    pub highlights: Vec<Highlight>,
    pub metadata: Option<FileMetadata>,
}

/// Text highlight
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Highlight {
    pub field: String,
    pub snippet: String,
    pub positions: Vec<(usize, usize)>,
}

/// File metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    pub size_bytes: u64,
    pub modified_at: String,
    pub created_at: Option<String>,
    pub permissions: Option<String>,
}

/// Chat message from hub to agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub chat_id: String,
    pub message: String,
    pub context: Option<serde_json::Value>,
}

/// Chat response from agent to hub
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub chat_id: String,
    pub response: String,
    pub sources: Vec<FileResult>,
    pub error: Option<String>,
}

/// Error message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorMessage {
    pub error_code: String,
    pub error_message: String,
    pub details: Option<HashMap<String, String>>,
}

/// Disconnect message
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct DisconnectMessage {
    pub session_id: String,
    pub reason: String,
    pub message: Option<String>,
}

impl MessageEnvelope {
    pub fn new<T: Serialize>(
        msg_type: MessageType,
        payload: &T,
    ) -> Result<Self, serde_json::Error> {
        Ok(Self {
            msg_type,
            payload: serde_json::to_value(payload)?,
            timestamp: chrono::Utc::now().to_rfc3339(),
        })
    }
}
