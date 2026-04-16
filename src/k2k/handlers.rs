//! K2K HTTP Endpoint Handlers

use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::sse::{Event, KeepAlive, Sse},
    Extension, Json,
};
use futures::stream::Stream;
use serde::Deserialize;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;
use tracing::{error, info, warn};
use uuid::Uuid;

use super::capabilities::ServiceManifest;
use super::models::*;
use super::server::K2KServerState;
use crate::config::load_capability_mesh_config;
use crate::connectors::web_clip::WebClipRequest;
use crate::store as db;
use crate::vectordb::VectorSearchResult;

// ============================================================================
// Health Check
// ============================================================================

pub async fn handle_health(State(state): State<Arc<K2KServerState>>) -> Json<HealthResponse> {
    let indexed_files = state.vectordb.count().await.unwrap_or(0);

    let uptime_seconds = state.start_time.elapsed().as_secs();

    Json(HealthResponse {
        status: "healthy",
        node_id: state.config.device.id.clone(),
        node_type: "system_agent",
        capabilities: state.capability_registry.capability_ids(),
        indexed_files,
        uptime_seconds,
    })
}

// ============================================================================
// Capabilities (Phase 5 — Multi-Agent Coordination)
// ============================================================================

pub async fn handle_list_capabilities(
    State(state): State<Arc<K2KServerState>>,
) -> Json<k2k::CapabilitiesResponse> {
    Json(state.capability_registry.get_response())
}

// ============================================================================
// Task Delegation (Phase 5 — Multi-Agent Coordination)
// ============================================================================

pub async fn handle_submit_task(
    State(state): State<Arc<K2KServerState>>,
    Extension(claims): Extension<K2KClaims>,
    Json(mut req): Json<k2k::TaskRequest>,
) -> Result<(StatusCode, HeaderMap, Json<k2k::TaskSubmitResponse>), (StatusCode, Json<K2KError>)> {
    // Inject the authenticated client_id from JWT claims so the task queue
    // can apply per-client rate limiting and storage quota checks.
    req.client_id = claims.client_id.clone();

    info!(
        "[TASKS] Task submission for capability '{}' from client '{}'",
        req.capability_id, req.client_id
    );

    match state.task_queue.submit(req).await {
        Ok(response) => Ok((StatusCode::ACCEPTED, HeaderMap::new(), Json(response))),
        Err(e) => {
            let msg = e.to_string();

            // Check for rate-limit sentinel: "RATE_LIMITED:<retry_after>:<message>"
            if let Some(rest) = msg.strip_prefix("RATE_LIMITED:") {
                let retry_after: u64 = rest
                    .split(':')
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(60);
                warn!("[TASKS] Rate-limited client '{}': retry after {}s", claims.client_id, retry_after);
                let mut headers = HeaderMap::new();
                if let Ok(val) = HeaderValue::from_str(&retry_after.to_string()) {
                    headers.insert("Retry-After", val);
                }
                return Err((
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(K2KError::rate_limited(
                        format!("Per-client rate limit exceeded. Retry after {}s.", retry_after),
                        retry_after,
                    )),
                ));
            }

            // Check for storage quota sentinel: "QUOTA_EXCEEDED:<message>"
            if msg.starts_with("QUOTA_EXCEEDED:") {
                warn!("[TASKS] Storage quota exceeded for client '{}'", claims.client_id);
                return Err((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(K2KError::bad_request(
                        msg.strip_prefix("QUOTA_EXCEEDED:").unwrap_or(&msg),
                    )),
                ));
            }

            warn!("[TASKS] Task submission failed: {}", msg);
            Err((
                StatusCode::BAD_REQUEST,
                Json(K2KError::bad_request(msg)),
            ))
        }
    }
}

pub async fn handle_get_task(
    State(state): State<Arc<K2KServerState>>,
    Extension(claims): Extension<K2KClaims>,
    Path(task_id): Path<String>,
) -> Result<Json<k2k::TaskStatusResponse>, (StatusCode, Json<K2KError>)> {
    match state.task_queue.get_status_for_client(&task_id, &claims.client_id).await {
        Some(status) => Ok(Json(status)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(K2KError::bad_request("Task not found")),
        )),
    }
}

pub async fn handle_cancel_task(
    State(state): State<Arc<K2KServerState>>,
    Extension(claims): Extension<K2KClaims>,
    Path(task_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<K2KError>)> {
    match state.task_queue.cancel_for_client(&task_id, &claims.client_id).await {
        Some(true) => Ok(StatusCode::NO_CONTENT),
        Some(false) => Err((
            StatusCode::CONFLICT,
            Json(K2KError::bad_request("Task is not in a cancellable state")),
        )),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(K2KError::bad_request("Task not found")),
        )),
    }
}

pub async fn handle_task_events(
    State(state): State<Arc<K2KServerState>>,
    Extension(claims): Extension<K2KClaims>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.task_queue.subscribe();
    let subscriber_client_id = claims.client_id.clone();

    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    // Only deliver events that belong to this client.
                    if event.client_id != subscriber_client_id {
                        continue;
                    }
                    let data = serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string());
                    let event_type = match event.event_type {
                        k2k::TaskEventType::StatusChanged => "status_changed",
                        k2k::TaskEventType::Progress => "progress",
                        k2k::TaskEventType::Completed => "completed",
                        k2k::TaskEventType::Failed => "failed",
                    };
                    yield Ok(Event::default().event(event_type).data(data));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!("[SSE] Skipped {} events", n);
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }
    };

    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ============================================================================
// Node Info
// ============================================================================

pub async fn handle_info(State(state): State<Arc<K2KServerState>>) -> Json<NodeInfo> {
    let public_key = state.key_manager.lock().await.get_public_key_pem();

    Json(NodeInfo {
        node_id: state.config.device.id.clone(),
        node_name: state.config.device.name.clone(),
        node_type: "system_agent",
        version: env!("CARGO_PKG_VERSION"),
        public_key,
        federation_endpoint: format!("http://localhost:{}/k2k/v1", state.config.k2k.port),
    })
}

// ============================================================================
// Client Registration
// ============================================================================

pub async fn handle_register_client(
    State(state): State<Arc<K2KServerState>>,
    Json(req): Json<RegisterClientRequest>,
) -> Result<Json<RegisterClientResponse>, (StatusCode, Json<K2KError>)> {
    // Determine approval status from registration policy.
    let registration_status = if !state.config.k2k.require_client_registration {
        // Registration gate disabled — still require approval rather than silently
        // auto-approving, which would let any client bypass auth entirely.
        warn!(
            "require_client_registration is disabled — client '{}' set to pending. \
             Enable require_client_registration or configure a registration_secret \
             to control access.",
            req.client_id
        );
        "pending"
    } else {
        match &state.config.k2k.registration_secret {
            Some(server_secret) => {
                // PSK is configured — use constant-time comparison to check secret
                use subtle::ConstantTimeEq;
                match &req.registration_secret {
                    Some(client_secret)
                        if client_secret.as_bytes().ct_eq(server_secret.as_bytes()).into() =>
                    {
                        "approved"
                    }
                    _ => "pending",
                }
            }
            None => {
                // No PSK configured — require manual approval
                "pending"
            }
        }
    };

    let mut key_manager = state.key_manager.lock().await;

    key_manager
        .register_client(
            req.client_id.clone(),
            req.client_name,
            req.public_key_pem,
            registration_status,
        )
        .await
        .map_err(|e| {
            error!("Failed to register client {}: {}", req.client_id, e);
            (
                StatusCode::BAD_REQUEST,
                Json(K2KError::bad_request(format!(
                    "Failed to register client: {}",
                    e
                ))),
            )
        })?;

    let message = match registration_status {
        "approved" => "Client registered and auto-approved".to_string(),
        _ => "Client registered, pending admin approval".to_string(),
    };

    info!(
        "Registered K2K client: {} (status: {})",
        req.client_id, registration_status
    );

    Ok(Json(RegisterClientResponse {
        status: registration_status.to_string(),
        client_id: req.client_id,
        message,
    }))
}

// ============================================================================
// Query Endpoint (Legacy)
// ============================================================================

pub async fn handle_query(
    State(state): State<Arc<K2KServerState>>,
    Extension(claims): Extension<K2KClaims>,
    Json(req): Json<K2KQueryRequest>,
) -> Result<Json<K2KQueryResponse>, (StatusCode, Json<K2KError>)> {
    let start = Instant::now();

    // Validate query length
    const MAX_QUERY_LENGTH: usize = 10_000;
    if req.query.len() > MAX_QUERY_LENGTH {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(K2KError::bad_request(format!(
                "Query exceeds maximum length of {} characters",
                MAX_QUERY_LENGTH
            ))),
        ));
    }

    // JWT is already verified by the middleware; claims are injected via Extension.
    info!(
        "K2K query from client '{}': \"{}\"",
        claims.client_id, req.query
    );

    // Check if client is allowed (if whitelist is configured)
    if !state.config.k2k.allowed_clients.is_empty()
        && !state.config.k2k.allowed_clients.contains(&claims.client_id)
    {
        warn!("Client '{}' not in allowed_clients list", claims.client_id);
        return Err((
            StatusCode::FORBIDDEN,
            Json(K2KError::forbidden(format!(
                "Client '{}' not allowed",
                claims.client_id
            ))),
        ));
    }

    // Validate paths against whitelist (if specified in filters)
    if let Some(ref filters) = req.filters {
        if let Some(ref paths) = filters.paths {
            for path in paths {
                if !is_path_allowed(&state.config.security.allowed_paths, path) {
                    warn!("Client requested forbidden path: {}", path);
                    return Err((
                        StatusCode::FORBIDDEN,
                        Json(K2KError::forbidden(format!("Path not allowed: {}", path))),
                    ));
                }
            }
        }
    }

    // 5. Generate embedding for query
    let query_embedding = {
        let mut model = state.embedding_model.lock().await;
        model.embed_text(&req.query).map_err(|e| {
            error!("Embedding generation failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(format!(
                    "Embedding generation failed: {}",
                    e
                ))),
            )
        })?
    };

    // 6. Perform semantic search via vectordb (cap top_k to prevent unbounded memory use)
    let results = state
        .vectordb
        .search(&query_embedding, req.top_k.min(100))
        .await
        .map_err(|e| {
            error!("Vector search failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(format!("Search failed: {}", e))),
            )
        })?;

    // 7. Filter results by path if specified in filters
    let filtered_results: Vec<VectorSearchResult> = if let Some(ref filters) = req.filters {
        if let Some(ref paths) = filters.paths {
            results
                .into_iter()
                .filter(|r| {
                    let result_path = std::path::Path::new(&r.path);
                    paths
                        .iter()
                        .any(|p| crate::path_utils::path_starts_with(result_path, std::path::Path::new(p)))
                })
                .collect()
        } else {
            results
        }
    } else {
        results
    };

    // 8. Format as K2K response
    let query_time_ms = start.elapsed().as_millis() as u64;

    let k2k_results: Vec<K2KResult> = filtered_results
        .into_iter()
        .map(|r| K2KResult {
            article_id: r.id.clone(),
            store_id: state.config.device.id.clone(),
            title: r.title.clone(),
            summary: r
                .chunk_text
                .clone()
                .map(|t| truncate_content(&t, 200))
                .unwrap_or_default(),
            content: r.chunk_text.unwrap_or_default(),
            confidence: r.score,
            source_type: r.source_type.clone(),
            tags: vec![],
            metadata: serde_json::json!({
                "path": r.path,
                "size_bytes": r.size_bytes,
                "modified_at": r.modified_at,
                "content_type": r.content_type,
                "document_type": r.document_type,
                "chunk_index": r.chunk_index,
            }),
            provenance: None,
        })
        .collect();

    info!(
        "Returned {} results in {}ms",
        k2k_results.len(),
        query_time_ms
    );

    Ok(Json(K2KQueryResponse {
        query_id: Uuid::new_v4().to_string(),
        total_results: k2k_results.len(),
        results: k2k_results,
        stores_queried: vec![state.config.device.id.clone()],
        query_time_ms,
        routing_decision: None,
        trace_id: req.trace_id.clone(),
    }))
}

// ============================================================================
// Routed Query (Phase 2)
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct RoutedQueryRequest {
    pub query: String,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
}

fn default_top_k() -> usize {
    10
}

pub async fn handle_routed_query(
    State(state): State<Arc<K2KServerState>>,
    Json(req): Json<RoutedQueryRequest>,
) -> Result<Json<K2KQueryResponse>, (StatusCode, Json<K2KError>)> {
    // Use default owner user if not specified
    let user_id = match req.user_id {
        Some(id) => id,
        None => state
            .db
            .get_owner_user()
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(K2KError::internal_error(e.to_string())),
                )
            })?
            .map(|u| u.id)
            .unwrap_or_default(),
    };

    let response = state
        .router
        .route(&req.query, &user_id, req.context.as_deref(), req.top_k.min(100))
        .await
        .map_err(|e| {
            error!("Router query failed: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(format!("Router failed: {}", e))),
            )
        })?;

    Ok(Json(response))
}

// ============================================================================
// List Stores (Phase 2)
// ============================================================================

pub async fn handle_list_stores(
    State(state): State<Arc<K2KServerState>>,
    Extension(claims): Extension<K2KClaims>,
) -> Result<Json<Vec<db::KnowledgeStore>>, (StatusCode, Json<K2KError>)> {
    let stores = state.db.list_stores_for_user(&claims.client_id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(K2KError::internal_error(e.to_string())),
        )
    })?;
    Ok(Json(stores))
}

// ============================================================================
// Article CRUD (Phase 3)
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateArticleRequest {
    pub store_id: String,
    pub title: String,
    pub content: String,
    #[serde(default = "default_source_type")]
    pub source_type: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_source_type() -> String {
    "user".to_string()
}

pub async fn handle_create_article(
    State(state): State<Arc<K2KServerState>>,
    Extension(claims): Extension<K2KClaims>,
    Json(req): Json<CreateArticleRequest>,
) -> Result<(StatusCode, Json<db::Article>), (StatusCode, Json<K2KError>)> {
    const MAX_TITLE_LEN: usize = 1_000;
    const MAX_CONTENT_LEN: usize = 1_500_000;
    if req.title.len() > MAX_TITLE_LEN {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, Json(K2KError::bad_request("Title exceeds maximum length"))));
    }
    if req.content.len() > MAX_CONTENT_LEN {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, Json(K2KError::bad_request("Content exceeds maximum length"))));
    }

    let store = state
        .db
        .get_store(&req.store_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(K2KError::bad_request("Store not found")),
            )
        })?;

    // ACL: verify the client may write to this store.
    // Personal stores are always writable by their owner.
    // All other store types require an explicit grant in config.
    let client_may_write = if store.store_type == "personal" && store.owner_id == claims.client_id {
        true
    } else {
        state
            .config
            .k2k
            .client_store_grants
            .get(&claims.client_id)
            .map(|grants| grants.iter().any(|id| id == &req.store_id))
            .unwrap_or(false)
    };

    if !client_may_write {
        warn!(
            "[ACL] Client '{}' denied write access to store '{}'",
            claims.client_id, req.store_id
        );
        return Err((
            StatusCode::FORBIDDEN,
            Json(K2KError::forbidden(format!(
                "Client '{}' does not have write permission for store '{}'",
                claims.client_id, req.store_id
            ))),
        ));
    }

    let now = chrono::Utc::now().to_rfc3339();
    let content_hash = crate::store::hash::content_hash(&req.content);
    let article = db::Article {
        id: Uuid::new_v4().to_string(),
        store_id: req.store_id,
        title: req.title,
        content: req.content,
        source_type: req.source_type,
        source_id: String::new(),
        content_hash,
        tags: serde_json::json!(req.tags),
        embedded_at: None,
        created_at: now.clone(),
        updated_at: now,
    };

    state
        .article_service
        .create(&article, &store.lancedb_collection)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?;

    Ok((StatusCode::CREATED, Json(article)))
}

pub async fn handle_get_article(
    State(state): State<Arc<K2KServerState>>,
    Extension(claims): Extension<K2KClaims>,
    Path(id): Path<String>,
) -> Result<Json<db::Article>, (StatusCode, Json<K2KError>)> {
    let article = state
        .article_service
        .get(&id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(K2KError::bad_request("Article not found")),
            )
        })?;

    // ACL: verify the client has read access via the article's parent store.
    // Return 404 for both not-found and unauthorized to avoid revealing existence.
    let store = state
        .db
        .get_store(&article.store_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(K2KError::bad_request("Article not found")),
            )
        })?;

    let client_may_read = if store.store_type == "personal" && store.owner_id == claims.client_id {
        true
    } else {
        state
            .config
            .k2k
            .client_store_grants
            .get(&claims.client_id)
            .map(|grants| grants.iter().any(|id| id == &article.store_id))
            .unwrap_or(false)
    };

    if !client_may_read {
        warn!(
            "[ACL] Client '{}' denied read access to article '{}' in store '{}'",
            claims.client_id, id, article.store_id
        );
        return Err((
            StatusCode::NOT_FOUND,
            Json(K2KError::bad_request("Article not found")),
        ));
    }

    Ok(Json(article))
}

#[derive(Debug, Deserialize)]
pub struct UpdateArticleRequest {
    pub title: Option<String>,
    pub content: Option<String>,
    pub tags: Option<Vec<String>>,
}

pub async fn handle_update_article(
    State(state): State<Arc<K2KServerState>>,
    Extension(claims): Extension<K2KClaims>,
    Path(id): Path<String>,
    Json(req): Json<UpdateArticleRequest>,
) -> Result<Json<db::Article>, (StatusCode, Json<K2KError>)> {
    const MAX_TITLE_LEN: usize = 1_000;
    const MAX_CONTENT_LEN: usize = 1_500_000;
    if let Some(ref t) = req.title {
        if t.len() > MAX_TITLE_LEN {
            return Err((StatusCode::UNPROCESSABLE_ENTITY, Json(K2KError::bad_request("Title exceeds maximum length"))));
        }
    }
    if let Some(ref c) = req.content {
        if c.len() > MAX_CONTENT_LEN {
            return Err((StatusCode::UNPROCESSABLE_ENTITY, Json(K2KError::bad_request("Content exceeds maximum length"))));
        }
    }

    let mut article = state
        .article_service
        .get(&id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(K2KError::bad_request("Article not found")),
            )
        })?;

    let store = state
        .db
        .get_store(&article.store_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(K2KError::bad_request("Store not found")),
            )
        })?;

    // ACL: verify the client may write to the store that owns this article.
    let client_may_write = if store.store_type == "personal" && store.owner_id == claims.client_id {
        true
    } else {
        state
            .config
            .k2k
            .client_store_grants
            .get(&claims.client_id)
            .map(|grants| grants.iter().any(|id| id == &article.store_id))
            .unwrap_or(false)
    };

    if !client_may_write {
        warn!(
            "[ACL] Client '{}' denied update access to article '{}' in store '{}'",
            claims.client_id, id, article.store_id
        );
        return Err((
            StatusCode::FORBIDDEN,
            Json(K2KError::forbidden(format!(
                "Client '{}' does not have write permission for store '{}'",
                claims.client_id, article.store_id
            ))),
        ));
    }

    if let Some(title) = req.title {
        article.title = title;
    }
    if let Some(content) = req.content {
        article.content = content;
    }
    if let Some(tags) = req.tags {
        article.tags = serde_json::json!(tags);
    }
    article.updated_at = chrono::Utc::now().to_rfc3339();

    state
        .article_service
        .update(&article, &store.lancedb_collection)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?;

    Ok(Json(article))
}

pub async fn handle_delete_article(
    State(state): State<Arc<K2KServerState>>,
    Extension(claims): Extension<K2KClaims>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<K2KError>)> {
    // Verify write access via the article's parent store before deleting.
    // Return 404 for both not-found and unauthorized to avoid revealing existence.
    let article = state
        .article_service
        .get(&id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(K2KError::bad_request("Article not found")),
            )
        })?;

    let store = state
        .db
        .get_store(&article.store_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(K2KError::bad_request("Article not found")),
            )
        })?;

    let client_may_write = if store.store_type == "personal" && store.owner_id == claims.client_id {
        true
    } else {
        state
            .config
            .k2k
            .client_store_grants
            .get(&claims.client_id)
            .map(|grants| grants.iter().any(|id| id == &article.store_id))
            .unwrap_or(false)
    };

    if !client_may_write {
        warn!(
            "[ACL] Client '{}' denied delete access to article '{}' in store '{}'",
            claims.client_id, id, article.store_id
        );
        return Err((
            StatusCode::NOT_FOUND,
            Json(K2KError::bad_request("Article not found")),
        ));
    }

    state.article_service.delete(&id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(K2KError::internal_error(e.to_string())),
        )
    })?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn handle_list_store_articles(
    State(state): State<Arc<K2KServerState>>,
    Extension(claims): Extension<K2KClaims>,
    Path(store_id): Path<String>,
) -> Result<Json<Vec<db::Article>>, (StatusCode, Json<K2KError>)> {
    // Verify access to the store before listing its articles.
    let store = state
        .db
        .get_store(&store_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(K2KError::bad_request("Store not found")),
            )
        })?;

    let client_may_read = if store.store_type == "personal" && store.owner_id == claims.client_id {
        true
    } else {
        state
            .config
            .k2k
            .client_store_grants
            .get(&claims.client_id)
            .map(|grants| grants.iter().any(|id| id == &store_id))
            .unwrap_or(false)
    };

    if !client_may_read {
        warn!(
            "[ACL] Client '{}' denied list access to store '{}'",
            claims.client_id, store_id
        );
        return Err((
            StatusCode::NOT_FOUND,
            Json(K2KError::bad_request("Store not found")),
        ));
    }

    let articles = state
        .article_service
        .list_for_store(&store_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?;
    Ok(Json(articles))
}

// ============================================================================
// Conversation Endpoints (Phase 3)
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct CreateConversationRequest {
    pub user_id: String,
    #[serde(default = "default_conv_title")]
    pub title: String,
}

fn default_conv_title() -> String {
    "New Conversation".to_string()
}

pub async fn handle_create_conversation(
    State(state): State<Arc<K2KServerState>>,
    Extension(claims): Extension<K2KClaims>,
    Json(req): Json<CreateConversationRequest>,
) -> Result<(StatusCode, Json<db::Conversation>), (StatusCode, Json<K2KError>)> {
    let now = chrono::Utc::now().to_rfc3339();
    let conv = db::Conversation {
        id: Uuid::new_v4().to_string(),
        // Enforce server-side ownership — never trust user_id from request body.
        user_id: claims.client_id,
        title: req.title,
        message_count: 0,
        created_at: now.clone(),
        updated_at: now,
    };

    state.conversation_service.create(&conv).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(K2KError::internal_error(e.to_string())),
        )
    })?;

    Ok((StatusCode::CREATED, Json(conv)))
}

#[derive(Debug, serde::Serialize)]
pub struct ConversationWithMessages {
    #[serde(flatten)]
    pub conversation: db::Conversation,
    pub messages: Vec<db::Message>,
}

pub async fn handle_get_conversation(
    State(state): State<Arc<K2KServerState>>,
    Extension(claims): Extension<K2KClaims>,
    Path(id): Path<String>,
) -> Result<Json<ConversationWithMessages>, (StatusCode, Json<K2KError>)> {
    let conv = state
        .conversation_service
        .get(&id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(K2KError::bad_request("Conversation not found")),
            )
        })?;

    // Enforce ownership — return 404 for both not-found and not-owned to
    // avoid revealing the existence of other clients' conversations.
    if conv.user_id != claims.client_id {
        return Err((
            StatusCode::NOT_FOUND,
            Json(K2KError::bad_request("Conversation not found")),
        ));
    }

    let messages = state.conversation_service.get_messages(&id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(K2KError::internal_error(e.to_string())),
        )
    })?;

    Ok(Json(ConversationWithMessages {
        conversation: conv,
        messages,
    }))
}

#[derive(Debug, Deserialize)]
pub struct AddMessageRequest {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
}

pub async fn handle_add_message(
    State(state): State<Arc<K2KServerState>>,
    Extension(claims): Extension<K2KClaims>,
    Path(conversation_id): Path<String>,
    Json(req): Json<AddMessageRequest>,
) -> Result<(StatusCode, Json<db::Message>), (StatusCode, Json<K2KError>)> {
    // Verify ownership before mutating the conversation.
    let conv = state
        .conversation_service
        .get(&conversation_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(K2KError::bad_request("Conversation not found")),
            )
        })?;
    if conv.user_id != claims.client_id {
        return Err((
            StatusCode::NOT_FOUND,
            Json(K2KError::bad_request("Conversation not found")),
        ));
    }

    let now = chrono::Utc::now().to_rfc3339();
    let msg = db::Message {
        id: Uuid::new_v4().to_string(),
        conversation_id,
        role: req.role,
        content: req.content,
        metadata: req.metadata.unwrap_or(serde_json::json!({})),
        created_at: now,
    };

    state.conversation_service.add_message(&msg).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(K2KError::internal_error(e.to_string())),
        )
    })?;

    Ok((StatusCode::CREATED, Json(msg)))
}

pub async fn handle_list_user_conversations(
    State(state): State<Arc<K2KServerState>>,
    Extension(claims): Extension<K2KClaims>,
    Path(_user_id): Path<String>,
) -> Result<Json<Vec<db::Conversation>>, (StatusCode, Json<K2KError>)> {
    // Use authenticated client_id from JWT — ignore path parameter to prevent
    // clients from listing other users' conversations.
    let convs = state
        .conversation_service
        .list_for_user(&claims.client_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?;
    Ok(Json(convs))
}

// ============================================================================
// Knowledge Extraction (Phase 3)
// ============================================================================

#[derive(Debug, Deserialize)]
pub struct ExtractKnowledgeRequest {
    pub store_id: String,
    #[serde(default)]
    pub title: Option<String>,
}

pub async fn handle_extract_knowledge(
    State(state): State<Arc<K2KServerState>>,
    Extension(claims): Extension<K2KClaims>,
    Path(conversation_id): Path<String>,
    Json(req): Json<ExtractKnowledgeRequest>,
) -> Result<(StatusCode, Json<db::Article>), (StatusCode, Json<K2KError>)> {
    // Verify conversation ownership before extracting knowledge.
    let conv = state
        .conversation_service
        .get(&conversation_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(K2KError::bad_request("Conversation not found")),
            )
        })?;
    if conv.user_id != claims.client_id {
        return Err((
            StatusCode::NOT_FOUND,
            Json(K2KError::bad_request("Conversation not found")),
        ));
    }
    drop(conv);

    let store = state
        .db
        .get_store(&req.store_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(K2KError::bad_request("Store not found")),
            )
        })?;

    let article = state
        .knowledge_extractor
        .extract_from_conversation(
            &conversation_id,
            &req.store_id,
            &store.lancedb_collection,
            req.title,
        )
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?;

    Ok((StatusCode::CREATED, Json(article)))
}

// ============================================================================
// Admin Endpoints
// ============================================================================

pub async fn handle_list_pending_clients(
    State(state): State<Arc<K2KServerState>>,
) -> Result<Json<Vec<db::K2KClient>>, (StatusCode, Json<K2KError>)> {
    let clients = state.db.list_pending_k2k_clients().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(K2KError::internal_error(e.to_string())),
        )
    })?;
    Ok(Json(clients))
}

#[derive(Debug, Deserialize)]
pub struct ClientIdRequest {
    pub client_id: String,
}

pub async fn handle_approve_client(
    State(state): State<Arc<K2KServerState>>,
    Json(req): Json<ClientIdRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<K2KError>)> {
    state
        .db
        .update_k2k_client_status(&req.client_id, "approved")
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?;

    info!("Admin approved K2K client: {}", req.client_id);

    Ok(Json(serde_json::json!({
        "status": "approved",
        "client_id": req.client_id,
        "message": "Client approved successfully"
    })))
}

pub async fn handle_reject_client(
    State(state): State<Arc<K2KServerState>>,
    Json(req): Json<ClientIdRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<K2KError>)> {
    state
        .db
        .update_k2k_client_status(&req.client_id, "rejected")
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?;

    info!("Admin rejected K2K client: {}", req.client_id);

    Ok(Json(serde_json::json!({
        "status": "rejected",
        "client_id": req.client_id,
        "message": "Client rejected"
    })))
}

// ============================================================================
// Helper Functions
// ============================================================================

fn is_path_allowed(allowed_paths: &[String], requested_path: &str) -> bool {
    if allowed_paths.is_empty() {
        return false; // No whitelist = deny all
    }

    let requested = std::path::Path::new(requested_path);

    for allowed in allowed_paths {
        let allowed_path = shellexpand::tilde(allowed).to_string();
        let allowed_path = std::path::Path::new(&allowed_path);

        if crate::path_utils::path_starts_with(requested, allowed_path) {
            return true;
        }
    }

    false
}

fn truncate_content(content: &str, max_len: usize) -> String {
    if content.len() <= max_len {
        content.to_string()
    } else {
        // Walk back from max_len to find a valid UTF-8 char boundary
        let end = (0..=max_len.min(content.len()))
            .rev()
            .find(|&i| content.is_char_boundary(i))
            .unwrap_or(0);
        format!("{}...", &content[..end])
    }
}

// ============================================================================
// Discovery & Federation (Phase 4)
// ============================================================================

pub async fn handle_list_nodes(
    State(state): State<Arc<K2KServerState>>,
) -> Result<Json<Vec<db::DiscoveredNode>>, (StatusCode, Json<K2KError>)> {
    let nodes = state.db.list_discovered_nodes().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(K2KError::internal_error(e.to_string())),
        )
    })?;
    Ok(Json(nodes))
}

#[derive(Debug, Deserialize)]
pub struct CreateFederationRequest {
    pub local_store_id: String,
    pub remote_node_id: String,
    pub remote_endpoint: String,
    #[serde(default = "default_access_type")]
    pub access_type: String,
}

fn default_access_type() -> String {
    "read".to_string()
}

pub async fn handle_create_federation(
    State(state): State<Arc<K2KServerState>>,
    Extension(claims): Extension<K2KClaims>,
    Json(req): Json<CreateFederationRequest>,
) -> Result<(StatusCode, Json<db::FederationAgreement>), (StatusCode, Json<K2KError>)> {
    // Verify the local store exists and is owned by the requesting client.
    // Without the ownership check any authenticated client could federate
    // another client's store to a remote node they control (IDOR).
    let local_store = state
        .db
        .get_store(&req.local_store_id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(K2KError::bad_request("Store not found")),
            )
        })?;
    if local_store.owner_id != claims.client_id {
        return Err((
            StatusCode::NOT_FOUND,
            Json(K2KError::bad_request("Store not found")),
        ));
    }

    let agreement = db::FederationAgreement {
        id: Uuid::new_v4().to_string(),
        local_store_id: req.local_store_id,
        remote_node_id: req.remote_node_id,
        remote_endpoint: req.remote_endpoint,
        access_type: req.access_type,
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    state
        .db
        .create_federation_agreement(&agreement)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?;

    Ok((StatusCode::CREATED, Json(agreement)))
}

pub async fn handle_list_federations(
    State(state): State<Arc<K2KServerState>>,
    Extension(claims): Extension<K2KClaims>,
) -> Result<Json<Vec<db::FederationAgreement>>, (StatusCode, Json<K2KError>)> {
    // Only return federation agreements for stores owned by the requesting client.
    let client_stores = state.db.list_stores_for_user(&claims.client_id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(K2KError::internal_error(e.to_string())),
        )
    })?;
    let client_store_ids: std::collections::HashSet<String> =
        client_stores.into_iter().map(|s| s.id).collect();
    let all_agreements = state.db.list_federation_agreements().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(K2KError::internal_error(e.to_string())),
        )
    })?;
    let agreements = all_agreements
        .into_iter()
        .filter(|a| client_store_ids.contains(&a.local_store_id))
        .collect();
    Ok(Json(agreements))
}

// ============================================================================
// Connector Endpoints (Phase 5)
// ============================================================================

pub async fn handle_web_clip(
    State(state): State<Arc<K2KServerState>>,
    Extension(claims): Extension<K2KClaims>,
    Json(req): Json<WebClipRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<K2KError>)> {
    // If the caller supplied an explicit target store, verify they own it.
    // Without this check any authenticated client could inject content into
    // another client's store by specifying its store_id (IDOR).
    if let Some(ref store_id) = req.store_id {
        let store = state.db.get_store(store_id).await.map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(e.to_string())),
            )
        })?;
        match store {
            Some(s) if s.owner_id == claims.client_id => {}
            Some(_) => {
                return Err((
                    StatusCode::NOT_FOUND,
                    Json(K2KError::bad_request("Store not found")),
                ));
            }
            None => {
                return Err((
                    StatusCode::NOT_FOUND,
                    Json(K2KError::bad_request("Store not found")),
                ));
            }
        }
    }

    let response = state.web_clip_receiver.ingest(&req).await.map_err(|e| {
        error!("Web clip ingestion failed: {}", e);
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(K2KError::internal_error(e.to_string())),
        )
    })?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "article_id": response.article_id,
            "message": response.message,
        })),
    ))
}

#[derive(Debug, serde::Serialize)]
pub struct ConnectorInfo {
    pub connector_type: String,
    pub name: String,
    pub healthy: bool,
}

pub async fn handle_list_connectors(
    State(state): State<Arc<K2KServerState>>,
) -> Json<Vec<ConnectorInfo>> {
    let list = state.connector_registry.list().await;
    let connectors = list
        .into_iter()
        .map(|(ctype, name, healthy)| ConnectorInfo {
            connector_type: ctype,
            name,
            healthy,
        })
        .collect();
    Json(connectors)
}

// ============================================================================
// Capability Manifest — richer metadata endpoint
// ============================================================================

pub async fn handle_capability_manifest(
    State(state): State<Arc<K2KServerState>>,
) -> Json<serde_json::Value> {
    // Optionally run health checks before returning (best-effort, non-blocking in prod)
    let infos = state.capability_registry.list_full();
    Json(serde_json::json!({
        "node_id": state.config.device.id,
        "protocol_version": "1.0",
        "capabilities": infos,
        "total": infos.len(),
    }))
}

// ============================================================================
// Capability Refresh — hot-reload from capabilities.yaml
// ============================================================================

pub async fn handle_refresh_capabilities(
    State(state): State<Arc<K2KServerState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<K2KError>)> {
    match load_capability_mesh_config().await {
        Ok(mesh_cfg) => {
            let count = mesh_cfg.capabilities.len();
            let remote_pairs = state
                .capability_registry
                .load_from_config_entries(mesh_cfg.capabilities);
            for (cap_id, handler) in remote_pairs {
                info!(
                    "[capabilities/refresh] Registered remote handler for '{}'",
                    cap_id
                );
                state
                    .task_queue
                    .register_handler(&cap_id, std::sync::Arc::new(handler))
                    .await;
            }
            Ok(Json(serde_json::json!({
                "status": "refreshed",
                "loaded": count,
                "total_capabilities": state.capability_registry.list().len(),
            })))
        }
        Err(e) => {
            warn!("[capabilities/refresh] Failed to load capabilities.yaml: {}", e);
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(K2KError::internal_error(format!(
                    "Failed to reload capabilities: {}",
                    e
                ))),
            ))
        }
    }
}

// ============================================================================
// Service Auto-Discovery
// ============================================================================

pub async fn handle_discover_services(
    State(state): State<Arc<K2KServerState>>,
) -> Json<serde_json::Value> {
    // Load the list of service base URLs from capabilities.yaml discovery config,
    // supplemented by common local service ports.
    let mut service_urls: Vec<String> = vec![
        // Add your local service URLs here, e.g.:
        // "http://localhost:8001".to_string(),
    ];

    // Merge URLs from capabilities.yaml discovery config
    if let Ok(mesh_cfg) = load_capability_mesh_config().await {
        for url in mesh_cfg.discovery.service_base_urls {
            if !service_urls.contains(&url) {
                service_urls.push(url);
            }
        }
    }

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return Json(serde_json::json!({
                "status": "error",
                "error": format!("Failed to build HTTP client: {}", e),
                "discovered": 0,
            }));
        }
    };

    let mut discovered_count = 0usize;
    let mut service_results = vec![];

    for base_url in &service_urls {
        // Skip if still within TTL
        if !state.capability_registry.needs_rediscovery(base_url) {
            service_results.push(serde_json::json!({
                "base_url": base_url,
                "status": "cached",
            }));
            continue;
        }

        let manifest_url = format!("{}/.well-known/k2k-manifest", base_url.trim_end_matches('/'));
        match client.get(&manifest_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<ServiceManifest>().await {
                    Ok(manifest) => {
                        let cap_count = manifest.capabilities.len();
                        let service_name = manifest.service_name.clone();
                        let remote_pairs = state
                            .capability_registry
                            .register_discovered(base_url, manifest);
                        for (cap_id, handler) in remote_pairs {
                            state
                                .task_queue
                                .register_handler(&cap_id, std::sync::Arc::new(handler))
                                .await;
                        }
                        discovered_count += cap_count;
                        service_results.push(serde_json::json!({
                            "base_url": base_url,
                            "service_name": service_name,
                            "status": "discovered",
                            "capabilities_found": cap_count,
                        }));
                        info!(
                            "[discover-services] {} capabilities from {}",
                            cap_count, base_url
                        );
                    }
                    Err(e) => {
                        service_results.push(serde_json::json!({
                            "base_url": base_url,
                            "status": "parse_error",
                            "error": e.to_string(),
                        }));
                    }
                }
            }
            Ok(resp) => {
                service_results.push(serde_json::json!({
                    "base_url": base_url,
                    "status": "not_found",
                    "http_status": resp.status().as_u16(),
                }));
            }
            Err(e) => {
                service_results.push(serde_json::json!({
                    "base_url": base_url,
                    "status": "unreachable",
                    "error": e.to_string(),
                }));
            }
        }
    }

    Json(serde_json::json!({
        "status": "complete",
        "services_probed": service_urls.len(),
        "new_capabilities_discovered": discovered_count,
        "total_capabilities": state.capability_registry.list().len(),
        "services": service_results,
    }))
}

// ============================================================================
// Well-Known Manifest — advertise this node's capabilities to other services
// ============================================================================

pub async fn handle_wellknown_manifest(
    State(state): State<Arc<K2KServerState>>,
) -> Json<serde_json::Value> {
    let caps: Vec<serde_json::Value> = state
        .capability_registry
        .list_full()
        .into_iter()
        .filter(|c| c.handler_type == "local")
        .map(|c| {
            serde_json::json!({
                "id": c.capability.id,
                "name": c.capability.name,
                "category": c.category_label,
                "description": c.capability.description,
                "input_schema": c.capability.input_schema,
                "output_schema": c.output_schema,
                "endpoint": format!("/k2k/v1/tasks"),
                "method": "POST",
                "supports_streaming": c.supports_streaming,
            })
        })
        .collect();

    Json(serde_json::json!({
        "service_name": format!("system-agent-{}", state.config.device.id),
        "version": env!("CARGO_PKG_VERSION"),
        "capabilities": caps,
    }))
}
