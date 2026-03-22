//! WebSocket client for Agent Hub connection.

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tokio::time::{interval, sleep};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};

use super::messages::*;
use crate::config::Config;
use crate::search;

/// Connection state
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Authenticating,
    Connected,
    Reconnecting,
}

/// WebSocket client
pub struct HubClient {
    config: Config,
    state: Arc<RwLock<ConnectionState>>,
    session_id: Arc<RwLock<Option<String>>>,
    reconnect_attempt: Arc<RwLock<u32>>,
}

impl HubClient {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            session_id: Arc::new(RwLock::new(None)),
            reconnect_attempt: Arc::new(RwLock::new(0)),
        }
    }

    /// Get current connection state
    #[allow(dead_code)]
    pub async fn state(&self) -> ConnectionState {
        self.state.read().await.clone()
    }

    /// Check if connected
    #[allow(dead_code)]
    pub async fn is_connected(&self) -> bool {
        *self.state.read().await == ConnectionState::Connected
    }

    /// Run the connection loop
    pub async fn run(&self) -> Result<()> {
        loop {
            match self.connect_and_run().await {
                Ok(_) => {
                    info!("Connection closed normally");
                    break;
                }
                Err(e) => {
                    error!("Connection error: {}", e);

                    if !self.config.connection.reconnect.enabled {
                        return Err(e);
                    }

                    // Calculate backoff delay
                    let attempt = {
                        let mut attempt = self.reconnect_attempt.write().await;
                        *attempt += 1;
                        *attempt
                    };

                    let max_attempts = self.config.connection.reconnect.max_attempts;
                    if max_attempts > 0 && attempt >= max_attempts {
                        error!("Max reconnect attempts ({}) exceeded", max_attempts);
                        return Err(e);
                    }

                    let delay = self.calculate_backoff(attempt);
                    info!(
                        "Reconnecting in {:.1}s (attempt {})",
                        delay.as_secs_f32(),
                        attempt
                    );

                    *self.state.write().await = ConnectionState::Reconnecting;
                    sleep(delay).await;
                }
            }
        }

        Ok(())
    }

    /// Connect and run message loop
    async fn connect_and_run(&self) -> Result<()> {
        let hub_url = self
            .config
            .connection
            .hub_url
            .as_ref()
            .context("Hub URL not configured")?;

        *self.state.write().await = ConnectionState::Connecting;
        info!("Connecting to {}", hub_url);

        // Connect with timeout to prevent hanging indefinitely
        let (ws_stream, _response) =
            tokio::time::timeout(Duration::from_secs(30), connect_async(hub_url))
                .await
                .context("Connection timeout")?
                .context("Failed to connect to hub")?;

        let (mut write, mut read) = ws_stream.split();

        // Authenticate
        *self.state.write().await = ConnectionState::Authenticating;
        info!("Authenticating...");

        let auth_request = self.build_auth_request();
        let auth_msg = MessageEnvelope::new(MessageType::AuthRequest, &auth_request)?;
        write
            .send(Message::Text(serde_json::to_string(&auth_msg)?))
            .await?;

        // Wait for auth response
        let auth_response = tokio::time::timeout(Duration::from_secs(30), read.next())
            .await
            .context("Auth timeout")?
            .context("Connection closed during auth")?
            .context("WebSocket error during auth")?;

        let response = self.parse_auth_response(&auth_response)?;
        if !response.success {
            anyhow::bail!(
                "Authentication failed: {} - {}",
                response.error_code.unwrap_or_default(),
                response.error_message.unwrap_or_default()
            );
        }

        // Store session ID
        *self.session_id.write().await = response.session_id.clone();
        *self.state.write().await = ConnectionState::Connected;
        *self.reconnect_attempt.write().await = 0;

        info!("Connected! Session: {:?}", response.session_id);

        // Create channels for communication
        let (tx, mut rx) = mpsc::channel::<Message>(32);

        // Spawn heartbeat task
        let heartbeat_interval = response.heartbeat_interval_seconds;
        let session_id = response.session_id.clone();
        let tx_heartbeat = tx.clone();
        let heartbeat_handle = tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(heartbeat_interval as u64));
            loop {
                ticker.tick().await;
                if let Some(ref sid) = session_id {
                    let heartbeat = Heartbeat {
                        session_id: sid.clone(),
                        status: "idle".to_string(),
                        metrics: None,
                    };
                    let msg = match MessageEnvelope::new(MessageType::Heartbeat, &heartbeat) {
                        Ok(m) => m,
                        Err(e) => {
                            tracing::error!("Failed to create heartbeat envelope: {}", e);
                            break;
                        }
                    };
                    let serialized = match serde_json::to_string(&msg) {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("Failed to serialize heartbeat: {}", e);
                            break;
                        }
                    };
                    if tx_heartbeat.send(Message::Text(serialized)).await.is_err() {
                        break;
                    }
                    debug!("Heartbeat sent");
                }
            }
        });

        // Message loop
        loop {
            tokio::select! {
                // Outgoing messages
                Some(msg) = rx.recv() => {
                    write.send(msg).await?;
                }

                // Incoming messages
                Some(msg) = read.next() => {
                    match msg {
                        Ok(Message::Text(text)) => {
                            if let Err(e) = self.handle_message(&text, &tx).await {
                                error!("Error handling message: {}", e);
                            }
                        }
                        Ok(Message::Close(frame)) => {
                            if let Some(f) = frame {
                                info!("Server closed connection: code={}, reason={}", f.code, f.reason);
                            } else {
                                info!("Server closed connection");
                            }
                            break;
                        }
                        Ok(_) => {}
                        Err(e) => {
                            error!("WebSocket error: {}", e);
                            break;
                        }
                    }
                }

                else => break,
            }
        }

        heartbeat_handle.abort();
        *self.state.write().await = ConnectionState::Disconnected;

        Ok(())
    }

    /// Build authentication request
    fn build_auth_request(&self) -> AuthRequest {
        AuthRequest {
            protocol_version: "1.0".to_string(),
            auth_method: "jwt".to_string(),
            token: self
                .config
                .connection
                .auth_token
                .clone()
                .unwrap_or_default(),
            device: DeviceInfo {
                device_id: self.config.device.id.clone(),
                device_name: self.config.device.name.clone(),
                platform: std::env::consts::OS.to_string(),
                platform_version: "".to_string(),
                agent_version: env!("CARGO_PKG_VERSION").to_string(),
            },
            capabilities: vec![
                "file_search".to_string(),
                "file_read".to_string(),
                "directory_list".to_string(),
            ],
            allowed_paths: self.config.security.allowed_paths.clone(),
            blocked_patterns: self.config.security.blocked_patterns.clone(),
        }
    }

    /// Parse authentication response
    fn parse_auth_response(&self, msg: &Message) -> Result<AuthResponse> {
        let text = msg.to_text().context("Expected text message")?;
        let envelope: MessageEnvelope = serde_json::from_str(text)?;
        if !matches!(envelope.msg_type, MessageType::AuthResponse) {
            anyhow::bail!(
                "Expected auth_response message, got {:?}",
                envelope.msg_type
            );
        }
        let response: AuthResponse = serde_json::from_value(envelope.payload)?;
        Ok(response)
    }

    /// Handle incoming message
    async fn handle_message(&self, text: &str, tx: &mpsc::Sender<Message>) -> Result<()> {
        let envelope: MessageEnvelope = serde_json::from_str(text)?;

        match envelope.msg_type {
            MessageType::HeartbeatAck => {
                debug!("Heartbeat acknowledged");
            }
            MessageType::QueryRequest => {
                let request: QueryRequest = serde_json::from_value(envelope.payload)?;
                info!("Received query: {} ({})", request.query_id, request.query);

                // Execute query
                let response = self.execute_query(request).await;
                let msg = MessageEnvelope::new(MessageType::QueryResponse, &response)?;
                tx.send(Message::Text(serde_json::to_string(&msg)?)).await?;
            }
            MessageType::ChatMessage => {
                let request: ChatMessage = serde_json::from_value(envelope.payload)?;
                info!("Received chat message: {} - {}", request.chat_id, request.message);

                let response = self.handle_chat_message(request).await;
                let msg = MessageEnvelope::new(MessageType::ChatResponse, &response)?;
                tx.send(Message::Text(serde_json::to_string(&msg)?)).await?;
            }
            MessageType::Error => {
                let error: ErrorMessage = serde_json::from_value(envelope.payload)?;
                warn!(
                    "Received error: {} - {}",
                    error.error_code, error.error_message
                );
            }
            _ => {
                debug!("Unhandled message type: {:?}", envelope.msg_type);
            }
        }

        Ok(())
    }

    /// Handle a chat message — search local KB for context, format response.
    /// V1: returns search results as context without LLM generation.
    async fn handle_chat_message(&self, request: ChatMessage) -> ChatResponse {
        let search_result = search::search_files(&self.config, &request.message, 5).await;

        match search_result {
            Ok(results) => {
                if results.is_empty() {
                    return ChatResponse {
                        chat_id: request.chat_id,
                        response: "No relevant files found in your local knowledge base for this query.".to_string(),
                        sources: vec![],
                        error: None,
                    };
                }

                let source_list: Vec<FileResult> = results
                    .iter()
                    .map(|r| FileResult {
                        id: r.id.clone(),
                        path: r.path.clone(),
                        filename: r.filename.clone(),
                        content: r.snippet.clone().or_else(|| r.content.clone()),
                        content_type: r.content_type.clone(),
                        score: r.score,
                        highlights: vec![],
                        metadata: r.metadata.as_ref().map(|m| FileMetadata {
                            size_bytes: m.size_bytes,
                            modified_at: m.modified_at.clone(),
                            created_at: m.created_at.clone(),
                            permissions: m.permissions.clone(),
                        }),
                    })
                    .collect();

                let response_text = format!(
                    "Found {} relevant file(s) in your local knowledge base:\n{}",
                    source_list.len(),
                    source_list
                        .iter()
                        .map(|f| format!("• {} (score: {:.2})", f.filename, f.score))
                        .collect::<Vec<_>>()
                        .join("\n")
                );

                ChatResponse {
                    chat_id: request.chat_id,
                    response: response_text,
                    sources: source_list,
                    error: None,
                }
            }
            Err(e) => ChatResponse {
                chat_id: request.chat_id,
                response: "Error searching local knowledge base.".to_string(),
                sources: vec![],
                error: Some(e.to_string()),
            },
        }
    }

    /// Execute a query request
    async fn execute_query(&self, request: QueryRequest) -> QueryResponse {
        let start = std::time::Instant::now();
        let query_type = request.query_type.to_lowercase();
        let supported = [
            "semantic",
            "hybrid",
            "semantic_search",
            "semantic-search",
            "vector",
        ];
        if !supported.contains(&query_type.as_str()) {
            return QueryResponse {
                query_id: request.query_id,
                success: false,
                results: vec![],
                total_count: 0,
                returned_count: 0,
                execution_time_ms: start.elapsed().as_millis() as u64,
                error_code: Some("UNSUPPORTED_QUERY_TYPE".to_string()),
                error_message: Some(format!(
                    "Unsupported query_type '{}'; supported types: semantic, hybrid",
                    query_type
                )),
            };
        }

        let limit = request
            .filters
            .as_ref()
            .and_then(|f| f.max_results.map(|n| n.min(1000)))
            .unwrap_or(10);
        let include_content = request
            .options
            .as_ref()
            .and_then(|o| o.include_content)
            .unwrap_or(false);
        let include_metadata = request
            .options
            .as_ref()
            .and_then(|o| o.include_metadata)
            .unwrap_or(true);
        let include_highlights = request
            .options
            .as_ref()
            .and_then(|o| o.highlight_matches)
            .unwrap_or(false);
        let timeout_ms = request.timeout_ms.min(300_000); // cap at 5 minutes
        let query_id = request.query_id.clone();

        let search_result = if timeout_ms > 0 {
            tokio::time::timeout(
                Duration::from_millis(timeout_ms as u64),
                search::search_files(&self.config, &request.query, limit),
            )
            .await
            .map_err(|_| anyhow::anyhow!("Search timed out after {}ms", timeout_ms))
        } else {
            Ok(search::search_files(&self.config, &request.query, limit).await)
        };

        match search_result {
            Ok(results) => {
                let results = match results {
                    Ok(r) => r,
                    Err(e) => {
                        return QueryResponse {
                            query_id,
                            success: false,
                            results: vec![],
                            total_count: 0,
                            returned_count: 0,
                            execution_time_ms: start.elapsed().as_millis() as u64,
                            error_code: Some("SEARCH_ERROR".to_string()),
                            error_message: Some(e.to_string()),
                        };
                    }
                };
                let filtered = apply_query_filters(results, request.filters.as_ref());
                let file_results: Vec<FileResult> = filtered
                    .iter()
                    .map(|r| FileResult {
                        id: r.id.clone(),
                        path: r.path.clone(),
                        filename: r.filename.clone(),
                        content: if include_content {
                            r.content.clone().or_else(|| r.snippet.clone())
                        } else {
                            None
                        },
                        content_type: r.content_type.clone(),
                        score: r.score,
                        highlights: if include_highlights {
                            r.snippet
                                .as_ref()
                                .map(|s| {
                                    vec![Highlight {
                                        field: "content".to_string(),
                                        snippet: s.clone(),
                                        positions: vec![],
                                    }]
                                })
                                .unwrap_or_default()
                        } else {
                            vec![]
                        },
                        metadata: if include_metadata {
                            r.metadata.as_ref().map(|m| FileMetadata {
                                size_bytes: m.size_bytes,
                                modified_at: m.modified_at.clone(),
                                created_at: m.created_at.clone(),
                                permissions: m.permissions.clone(),
                            })
                        } else {
                            None
                        },
                    })
                    .collect();

                QueryResponse {
                    query_id,
                    success: true,
                    total_count: file_results.len(),
                    returned_count: file_results.len(),
                    results: file_results,
                    execution_time_ms: start.elapsed().as_millis() as u64,
                    error_code: None,
                    error_message: None,
                }
            }
            Err(e) => QueryResponse {
                query_id,
                success: false,
                results: vec![],
                total_count: 0,
                returned_count: 0,
                execution_time_ms: start.elapsed().as_millis() as u64,
                error_code: Some("SEARCH_TIMEOUT".to_string()),
                error_message: Some(e.to_string()),
            },
        }
    }

    /// Calculate exponential backoff delay
    fn calculate_backoff(&self, attempt: u32) -> Duration {
        let base = self.config.connection.reconnect.initial_delay;
        let max = self.config.connection.reconnect.max_delay;

        // Cap the exponent at 31 to prevent i32 overflow for large attempt counts.
        // The delay is also capped by max_delay below, so this only affects
        // the unlikely case of attempt > 31 where max_delay hasn't been reached.
        let exponent = (attempt.min(31)) as i32 - 1;
        let delay = base * 2.0_f32.powi(exponent);
        let delay = delay.min(max);

        // Add jitter (10%)
        let jitter = delay * 0.1 * rand::random::<f32>();
        Duration::from_secs_f32(delay + jitter)
    }
}

fn apply_query_filters(
    results: Vec<search::SearchResult>,
    filters: Option<&QueryFilters>,
) -> Vec<search::SearchResult> {
    let Some(filters) = filters else {
        return results;
    };

    results
        .into_iter()
        .filter(|r| {
            if let Some(paths) = &filters.paths {
                let result_path = std::path::Path::new(&r.path);
                if !paths
                    .iter()
                    .any(|p| crate::path_utils::path_starts_with(result_path, std::path::Path::new(p)))
                {
                    return false;
                }
            }

            if let Some(file_types) = &filters.file_types {
                let ext = std::path::Path::new(&r.path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s.to_lowercase());
                if !file_types.iter().any(|ft| {
                    let norm = ft.trim_start_matches('.').to_lowercase();
                    ext.as_deref() == Some(norm.as_str())
                }) {
                    return false;
                }
            }

            if let Some(max_size) = filters.max_file_size_bytes {
                if let Some(meta) = &r.metadata {
                    if meta.size_bytes > max_size {
                        return false;
                    }
                }
            }

            true
        })
        .collect()
}

/// Start the connection to the hub
pub async fn start_connection(config: Config) -> Result<()> {
    if config.connection.hub_url.is_none() {
        info!("Hub URL not configured, running in offline mode");
        // Keep running (don't return) so the select! doesn't terminate
        loop {
            sleep(Duration::from_secs(60)).await;
        }
    }

    if config
        .connection
        .auth_token
        .as_deref()
        .map(|t| t.trim().is_empty())
        .unwrap_or(true)
    {
        warn!("Hub URL is configured but auth token is missing; running in offline mode");
        loop {
            sleep(Duration::from_secs(60)).await;
        }
    }

    let client = HubClient::new(config);
    client.run().await
}
