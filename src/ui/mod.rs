//! Local web UI for offline search.
//!
//! Provides a simple web interface for searching files when offline,
//! plus a settings panel for managing indexed paths and hub connection.

mod assets;

use anyhow::Result;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Json, Response},
    routing::{delete, get, post, put},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::info;

use crate::config::{self, Config};
use crate::search;

/// Application state with mutable config and UI secret token
struct AppState {
    config: RwLock<Config>,
    /// Random token generated at startup.  Must be present in the
    /// `X-UI-Token` header on all mutation endpoints so that other local
    /// processes (different user account) cannot reconfigure the agent.
    ui_token: String,
}

/// Generate a random hex UI token using `rand` (already a workspace dependency).
fn generate_ui_token() -> String {
    use rand::Rng;
    let bytes: [u8; 16] = rand::thread_rng().gen();
    hex::encode(bytes)
}

/// Verify that the `X-UI-Token` header matches the server's token.
/// Returns `Err((403, …))` if the token is missing or wrong.
fn require_ui_token(
    headers: &HeaderMap,
    state: &AppState,
) -> Result<(), (StatusCode, Json<ApiResult>)> {
    let provided = headers
        .get("x-ui-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if provided != state.ui_token {
        Err((
            StatusCode::FORBIDDEN,
            Json(ApiResult {
                success: false,
                message: "Missing or invalid UI token".to_string(),
            }),
        ))
    } else {
        Ok(())
    }
}

/// Start the local web UI server
pub async fn start_server(config: Config) -> Result<()> {
    let port = config.ui.port;
    let ui_token = generate_ui_token();

    let state = Arc::new(AppState {
        config: RwLock::new(config),
        ui_token: ui_token.clone(),
    });

    // Restrict CORS to same-origin requests only (127.0.0.1:{port}).
    // This prevents browser-based CSRF attacks from external web pages while
    // preserving normal same-origin navigation from the UI itself.
    let allowed_origin = format!("http://127.0.0.1:{}", port);
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::exact(allowed_origin.parse().map_err(|e| {
            anyhow::anyhow!("Failed to parse allowed origin: {}", e)
        })?))
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any);

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/api/search", get(search_handler))
        .route("/api/status", get(status_handler))
        .route("/api/config", get(config_handler))
        .route("/api/paths", post(add_path_handler))
        .route("/api/paths", delete(remove_path_handler))
        .route("/api/hub", put(update_hub_handler))
        .route("/api/reindex", post(reindex_handler))
        .route("/assets/*path", get(assets_handler))
        .layer(cors)
        .with_state(state);

    let addr = format!("127.0.0.1:{}", port);
    info!("Starting local UI at http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Index page handler — injects the UI token into the HTML so the browser
/// can include it in mutation requests.
async fn index_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let html = assets::INDEX_HTML.replace("__UI_TOKEN_PLACEHOLDER__", &state.ui_token);
    Html(html)
}

/// Search query parameters
#[derive(Debug, Deserialize)]
struct SearchQuery {
    q: String,
    #[serde(default = "default_limit")]
    limit: usize,
}

fn default_limit() -> usize {
    10
}

/// Search result response
#[derive(Debug, Serialize)]
struct SearchResponse {
    success: bool,
    query: String,
    results: Vec<SearchResultItem>,
    count: usize,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct SearchResultItem {
    path: String,
    filename: String,
    score: f32,
    snippet: Option<String>,
    size_bytes: Option<u64>,
    modified_at: Option<String>,
}

/// Search API handler
async fn search_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<SearchQuery>,
) -> Json<SearchResponse> {
    let config = state.config.read().await;
    match search::search_files(&config, &query.q, query.limit).await {
        Ok(results) => {
            let items: Vec<SearchResultItem> = results
                .iter()
                .map(|r| SearchResultItem {
                    path: r.path.clone(),
                    filename: r.filename.clone(),
                    score: r.score,
                    snippet: r.snippet.clone(),
                    size_bytes: r.metadata.as_ref().map(|m| m.size_bytes),
                    modified_at: r.metadata.as_ref().map(|m| m.modified_at.clone()),
                })
                .collect();

            Json(SearchResponse {
                success: true,
                query: query.q,
                count: items.len(),
                results: items,
                error: None,
            })
        }
        Err(e) => Json(SearchResponse {
            success: false,
            query: query.q,
            results: vec![],
            count: 0,
            error: Some(e.to_string()),
        }),
    }
}

/// Status response
#[derive(Debug, Serialize)]
struct StatusResponse {
    status: String,
    device_name: String,
    device_id: String,
    indexed_paths: Vec<PathInfo>,
    hub_connected: bool,
    hub_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct PathInfo {
    path: String,
    exists: bool,
}

/// Status API handler
async fn status_handler(State(state): State<Arc<AppState>>) -> Json<StatusResponse> {
    let config = state.config.read().await;
    let hub_connected = config.connection.hub_url.is_some();

    let indexed_paths = config
        .security
        .allowed_paths
        .iter()
        .map(|p| PathInfo {
            exists: std::path::Path::new(p).is_dir(),
            path: p.clone(),
        })
        .collect();

    Json(StatusResponse {
        status: "running".to_string(),
        device_name: config.device.name.clone(),
        device_id: config.device.id.clone(),
        indexed_paths,
        hub_connected,
        hub_url: config.connection.hub_url.clone(),
    })
}

/// Config response
#[derive(Debug, Serialize)]
struct ConfigResponse {
    device: DeviceConfig,
    ui: UiConfig,
    indexing: IndexingConfig,
}

#[derive(Debug, Serialize)]
struct DeviceConfig {
    name: String,
    id: String,
}

#[derive(Debug, Serialize)]
struct UiConfig {
    port: u16,
    enabled: bool,
}

#[derive(Debug, Serialize)]
struct IndexingConfig {
    include_content: bool,
    max_file_size: u64,
}

/// Config API handler
async fn config_handler(State(state): State<Arc<AppState>>) -> Json<ConfigResponse> {
    let config = state.config.read().await;
    Json(ConfigResponse {
        device: DeviceConfig {
            name: config.device.name.clone(),
            id: config.device.id.clone(),
        },
        ui: UiConfig {
            port: config.ui.port,
            enabled: config.ui.enabled,
        },
        indexing: IndexingConfig {
            include_content: config.indexing.include_content,
            max_file_size: config.indexing.max_file_size,
        },
    })
}

// ---- Settings mutation endpoints ----

#[derive(Debug, Serialize)]
struct ApiResult {
    success: bool,
    message: String,
}

/// Add an indexed path
#[derive(Debug, Deserialize)]
struct AddPathRequest {
    path: String,
}

async fn add_path_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<AddPathRequest>,
) -> (StatusCode, Json<ApiResult>) {
    if let Err(e) = require_ui_token(&headers, &state) {
        return e;
    }
    let expanded = expand_path(&req.path);

    // Validate path exists
    if !std::path::Path::new(&expanded).is_dir() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ApiResult {
                success: false,
                message: format!("Directory does not exist: {}", expanded),
            }),
        );
    }

    let mut config = state.config.write().await;

    // Check if already added
    if config.security.allowed_paths.contains(&expanded) {
        return (
            StatusCode::CONFLICT,
            Json(ApiResult {
                success: false,
                message: format!("Path already configured: {}", expanded),
            }),
        );
    }

    config.security.allowed_paths.push(expanded.clone());

    // Save to disk
    if let Err(e) = config::save_config(&config).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResult {
                success: false,
                message: format!("Failed to save config: {}", e),
            }),
        );
    }

    info!("Path added via UI: {}", expanded);
    (
        StatusCode::OK,
        Json(ApiResult {
            success: true,
            message: format!("Added path: {}. Run reindex to index new files.", expanded),
        }),
    )
}

/// Remove an indexed path
#[derive(Debug, Deserialize)]
struct RemovePathRequest {
    path: String,
}

async fn remove_path_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<RemovePathRequest>,
) -> (StatusCode, Json<ApiResult>) {
    if let Err(e) = require_ui_token(&headers, &state) {
        return e;
    }
    let mut config = state.config.write().await;

    let original_len = config.security.allowed_paths.len();
    config
        .security
        .allowed_paths
        .retain(|p| p != &req.path);

    if config.security.allowed_paths.len() == original_len {
        return (
            StatusCode::NOT_FOUND,
            Json(ApiResult {
                success: false,
                message: format!("Path not found: {}", req.path),
            }),
        );
    }

    if let Err(e) = config::save_config(&config).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResult {
                success: false,
                message: format!("Failed to save config: {}", e),
            }),
        );
    }

    info!("Path removed via UI: {}", req.path);
    (
        StatusCode::OK,
        Json(ApiResult {
            success: true,
            message: format!("Removed path: {}", req.path),
        }),
    )
}

/// Update hub connection
#[derive(Deserialize)]
struct UpdateHubRequest {
    hub_url: Option<String>,
    auth_token: Option<String>,
}

impl std::fmt::Debug for UpdateHubRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdateHubRequest")
            .field("hub_url", &self.hub_url)
            .field("auth_token", &self.auth_token.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

async fn update_hub_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<UpdateHubRequest>,
) -> (StatusCode, Json<ApiResult>) {
    if let Err(e) = require_ui_token(&headers, &state) {
        return e;
    }
    let mut config = state.config.write().await;

    // Allow clearing hub_url with empty string or null
    config.connection.hub_url = req.hub_url.filter(|s| !s.is_empty());
    if let Some(token) = req.auth_token {
        config.connection.auth_token = if token.is_empty() { None } else { Some(token) };
    }

    if let Err(e) = config::save_config(&config).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResult {
                success: false,
                message: format!("Failed to save config: {}", e),
            }),
        );
    }

    let status = if config.connection.hub_url.is_some() {
        "Hub configured. Restart agent to connect."
    } else {
        "Hub disconnected."
    };

    info!("Hub config updated via UI");
    (
        StatusCode::OK,
        Json(ApiResult {
            success: true,
            message: status.to_string(),
        }),
    )
}

/// Trigger a reindex
async fn reindex_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> (StatusCode, Json<ApiResult>) {
    if let Err(e) = require_ui_token(&headers, &state) {
        return e;
    }
    let config = state.config.read().await.clone();

    // Spawn reindex in background so we can return immediately
    tokio::spawn(async move {
        info!("Reindex triggered from UI");
        if let Err(e) = search::reindex_all(&config, false).await {
            tracing::error!("Reindex failed: {}", e);
        }
    });

    (
        StatusCode::OK,
        Json(ApiResult {
            success: true,
            message: "Reindex started in background.".to_string(),
        }),
    )
}

/// Expand ~ in paths
fn expand_path(path: &str) -> String {
    if path.starts_with('~') {
        dirs::home_dir()
            .map(|home| path.replacen('~', &home.to_string_lossy(), 1))
            .unwrap_or_else(|| path.to_string())
    } else {
        path.to_string()
    }
}

/// Static assets handler
async fn assets_handler(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    match assets::get_asset(&path) {
        Some((content, content_type)) => {
            ([(axum::http::header::CONTENT_TYPE, content_type)], content).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
