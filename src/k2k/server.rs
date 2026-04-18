//! K2K HTTP Server

use axum::http::HeaderValue;
use axum::{
    routing::{delete as axum_delete, get, patch, post, put},
    Router,
};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

use super::capabilities::CapabilityRegistry;
use super::handlers::*;
use super::keys::KeyManager;
use super::middleware::require_k2k_auth;
use super::task_handlers::{SemanticSearchHandler, WebSearchHandler};
use super::tasks::{TaskQueue, TaskQueueLimits};
use crate::config::{load_capability_mesh_config, Config};
use crate::connectors::{ConnectorRegistry, WebClipReceiver};
use crate::store::Store;
use crate::discovery::NodeRegistry;
use crate::embeddings::EmbeddingModel;
use crate::federation::RemoteQueryExecutor;
use crate::knowledge::{ArticleService, ConversationService, KnowledgeExtractor};
use crate::retrieval::HybridSearcher;
use crate::router::LocalRouter;
use crate::vectordb::VectorDB;

pub struct K2KServerState {
    pub config: Config,
    pub key_manager: Mutex<KeyManager>,
    pub vectordb: Arc<VectorDB>,
    pub embedding_model: Arc<Mutex<EmbeddingModel>>,
    pub db: Arc<dyn Store>,
    pub router: Arc<LocalRouter>,
    pub article_service: Arc<ArticleService>,
    pub conversation_service: Arc<ConversationService>,
    pub knowledge_extractor: Arc<KnowledgeExtractor>,
    pub connector_registry: Arc<ConnectorRegistry>,
    pub web_clip_receiver: Arc<WebClipReceiver>,
    pub capability_registry: Arc<CapabilityRegistry>,
    pub task_queue: Arc<TaskQueue>,
    pub start_time: Instant,
}

pub struct K2KServer {
    state: Arc<K2KServerState>,
    port: u16,
}

impl K2KServer {
    pub async fn new(
        config: Config,
        config_dir: std::path::PathBuf,
        vectordb: Arc<VectorDB>,
        embedding_model: Arc<Mutex<EmbeddingModel>>,
        db: Arc<dyn Store>,
        connector_registry: Arc<ConnectorRegistry>,
        node_registry: Option<Arc<NodeRegistry>>,
    ) -> anyhow::Result<Self> {
        let key_manager = KeyManager::new(&config_dir, Some(db.clone())).await?;

        // Initialize retrieval components
        let hybrid_searcher = Some(Arc::new(HybridSearcher::new(db.clone())));

        // Initialize federation remote executor if node registry is available
        let remote_executor = if let Some(registry) = node_registry {
            Some(Arc::new(RemoteQueryExecutor::new(registry)?))
        } else {
            None
        };

        let router = Arc::new(LocalRouter::new(
            db.clone(),
            vectordb.clone(),
            embedding_model.clone(),
            hybrid_searcher,
            remote_executor,
        ));

        let article_service = Arc::new(ArticleService::new(
            db.clone(),
            vectordb.clone(),
            embedding_model.clone(),
            Some(config.extraction.clone()),
        ));

        let conversation_service = Arc::new(ConversationService::new(db.clone()));

        let knowledge_extractor =
            Arc::new(KnowledgeExtractor::new(db.clone(), article_service.clone()));

        let web_clip_receiver = Arc::new(WebClipReceiver::new(db.clone(), article_service.clone()));

        let capability_registry = Arc::new(CapabilityRegistry::new(config.device.id.clone()));

        let task_queue_limits = TaskQueueLimits {
            max_tasks_per_window: config.k2k.per_client_rate_limit,
            window_seconds: config.k2k.per_client_window_seconds,
            max_articles_per_client: config.k2k.per_client_article_quota,
        };
        let task_queue = Arc::new(TaskQueue::with_limits(4, task_queue_limits, Some(db.clone())));

        // Register built-in task handlers
        let search_handler = Arc::new(SemanticSearchHandler::new(
            vectordb.clone(),
            embedding_model.clone(),
        ));
        task_queue
            .register_handler("semantic_search", search_handler)
            .await;

        // Register web search handler
        let web_search_handler = Arc::new(WebSearchHandler::new(config.web_search.clone()));
        task_queue
            .register_handler("web_search", web_search_handler)
            .await;

        // Load capability mesh config (capabilities.yaml) and register remote handlers
        match load_capability_mesh_config().await {
            Ok(mesh_cfg) => {
                info!(
                    "[K2K] Loading {} capabilities from capabilities.yaml",
                    mesh_cfg.capabilities.len()
                );
                let remote_pairs =
                    capability_registry.load_from_config_entries(mesh_cfg.capabilities);
                for (cap_id, handler) in remote_pairs {
                    info!("[K2K] Registered remote handler for capability '{}'", cap_id);
                    task_queue
                        .register_handler(&cap_id, Arc::new(handler))
                        .await;
                }
            }
            Err(e) => {
                info!("[K2K] No capabilities.yaml or parse error ({}), using defaults only", e);
            }
        }

        let state = Arc::new(K2KServerState {
            config: config.clone(),
            key_manager: Mutex::new(key_manager),
            vectordb,
            embedding_model,
            db,
            router,
            article_service,
            conversation_service,
            knowledge_extractor,
            connector_registry,
            web_clip_receiver,
            capability_registry,
            task_queue,
            start_time: Instant::now(),
        });

        Ok(Self {
            state,
            port: config.k2k.port,
        })
    }

    pub async fn start(self) -> anyhow::Result<()> {
        // Public routes (no auth required)
        let public = Router::new()
            .route("/k2k/v1/health", get(handle_health))
            .route("/k2k/v1/info", get(handle_info))
            .route("/k2k/v1/register-client", post(handle_register_client))
            .route("/k2k/v1/capabilities", get(handle_list_capabilities))
            .route("/k2k/v1/capabilities/manifest", get(handle_capability_manifest))
            .route(
                "/k2k/v1/capabilities/refresh",
                get(handle_refresh_capabilities),
            )
            .route("/k2k/v1/discover-services", get(handle_discover_services))
            .route(
                "/.well-known/k2k-manifest",
                get(handle_wellknown_manifest),
            );

        // Admin routes (localhost only — inherently safe since we bind to 127.0.0.1)
        let admin = Router::new()
            .route(
                "/k2k/v1/admin/pending-clients",
                get(handle_list_pending_clients),
            )
            .route("/k2k/v1/admin/approve-client", post(handle_approve_client))
            .route("/k2k/v1/admin/reject-client", post(handle_reject_client));

        // Protected routes (JWT required)
        let protected = Router::new()
            .route("/k2k/v1/query", post(handle_query))
            .route("/k2k/v1/route", post(handle_routed_query))
            .route("/k2k/v1/stores", get(handle_list_stores))
            // Article API endpoints
            .route("/api/v1/articles", post(handle_create_article))
            .route("/api/v1/articles/:id", get(handle_get_article))
            .route("/api/v1/articles/:id", put(handle_update_article))
            .route("/api/v1/articles/:id", patch(handle_update_article))
            .route("/api/v1/articles/:id", axum_delete(handle_delete_article))
            .route(
                "/api/v1/stores/:id/articles",
                get(handle_list_store_articles),
            )
            // Conversation API endpoints
            .route("/api/v1/conversations", post(handle_create_conversation))
            .route("/api/v1/conversations/:id", get(handle_get_conversation))
            .route(
                "/api/v1/conversations/:id/messages",
                post(handle_add_message),
            )
            .route(
                "/api/v1/users/:id/conversations",
                get(handle_list_user_conversations),
            )
            .route(
                "/api/v1/conversations/:id/extract",
                post(handle_extract_knowledge),
            )
            // Discovery & Federation endpoints
            .route("/k2k/v1/nodes", get(handle_list_nodes))
            .route("/k2k/v1/federate", post(handle_create_federation))
            .route("/k2k/v1/federations", get(handle_list_federations))
            // Connector endpoints
            .route("/api/v1/web-clips", post(handle_web_clip))
            .route("/api/v1/connectors", get(handle_list_connectors))
            // Task delegation endpoints (Phase 5)
            .route("/k2k/v1/tasks", post(handle_submit_task))
            .route("/k2k/v1/tasks/events", get(handle_task_events))
            .route("/k2k/v1/tasks/:id", get(handle_get_task))
            .route("/k2k/v1/tasks/:id", axum_delete(handle_cancel_task))
            .layer(axum::middleware::from_fn_with_state(
                self.state.clone(),
                require_k2k_auth,
            ));

        // Build CORS layer
        let cors = build_cors_layer(&self.state.config);

        let app = public
            .merge(admin)
            .merge(protected)
            .with_state(self.state.clone())
            .layer(cors);

        let addr = format!("127.0.0.1:{}", self.port);
        info!("K2K server listening on {}", addr);
        info!(
            "   Federation endpoint: http://localhost:{}/k2k/v1",
            self.port
        );
        info!("   API endpoint: http://localhost:{}/api/v1", self.port);
        info!(
            "   Admin endpoint: http://localhost:{}/k2k/v1/admin",
            self.port
        );

        let listener = tokio::net::TcpListener::bind(&addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }
}

fn build_cors_layer(config: &Config) -> CorsLayer {
    if config.k2k.allowed_origins.is_empty() {
        // Default: localhost only
        let origins: Vec<HeaderValue> = [
            "http://localhost:5173",
            "http://localhost:19850",
            "http://127.0.0.1:5173",
            "http://127.0.0.1:19850",
        ]
        .iter()
        .filter_map(|o| o.parse().ok())
        .collect();

        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        // User-configured origins
        let origins: Vec<HeaderValue> = config
            .k2k
            .allowed_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();

        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods(Any)
            .allow_headers(Any)
    }
}
