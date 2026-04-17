//! Knowledge Nexus System Agent
//!
//! A secure, lightweight agent that runs on user devices to provide
//! local file access to the Knowledge Nexus AI system.
//!
//! Features:
//! - Semantic file search using embedded vector database
//! - Local-first with offline search capability
//! - System tray integration
//! - Secure path whitelist (cannot be overridden remotely)

mod config;
mod connection;
mod connectors;
pub mod constants;
mod migrate;
mod store;
mod discovery;
mod embeddings;
mod federation;
mod k2k;
mod knowledge;
pub(crate) mod path_utils;
mod retrieval;
mod router;
mod search;
mod security;
mod tray;
mod ui;
mod vectordb;

use anyhow::Result;
use clap::{Parser, Subcommand};
use store::Store;
use tracing::{info, Level};
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::FmtSubscriber;

#[derive(Parser)]
#[command(name = "knowledge-nexus-agent")]
#[command(author = "Jared Cluff")]
#[command(version)]
#[command(about = "Knowledge Nexus System Agent - Secure local file access for AI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize the agent configuration
    Init {
        /// Hub URL to connect to
        #[arg(long)]
        hub_url: Option<String>,

        /// Force overwrite existing configuration
        #[arg(long)]
        force: bool,
    },

    /// Start the agent
    Start {
        /// Run in foreground (don't daemonize)
        #[arg(long)]
        foreground: bool,

        /// Disable system tray icon
        #[arg(long)]
        no_tray: bool,
    },

    /// Stop the running agent
    Stop,

    /// Show agent status
    Status,

    /// Open configuration
    Config {
        /// Print configuration as JSON
        #[arg(long)]
        show: bool,

        /// Print configuration file path
        #[arg(long)]
        path: bool,

        /// Set a configuration value
        #[arg(long, value_name = "KEY=VALUE")]
        set: Option<String>,
    },

    /// Manage indexed paths
    Paths {
        #[command(subcommand)]
        action: PathsAction,
    },

    /// Search local files (offline mode)
    Search {
        /// Search query
        query: String,

        /// Maximum results
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },

    /// Reindex all files
    Reindex {
        /// Force full reindex (ignore cache)
        #[arg(long)]
        force: bool,

        /// Rebuild LanceDB index with a specific quantizer.
        /// Supported: ivf_pq_v1, int8_v1, turboquant_v1
        #[arg(long)]
        quantizer: Option<String>,

        /// Target store ID (required when --quantizer is set)
        #[arg(long)]
        store: Option<String>,
    },

    /// Open local search UI in browser
    Ui,

    /// Show connection status and logs
    Logs {
        /// Follow log output
        #[arg(short, long)]
        follow: bool,

        /// Number of lines to show
        #[arg(short, long, default_value = "50")]
        lines: usize,
    },

    /// Migrate a 0.8 SQLite database into 1.0.0 SurrealDB format.
    Migrate {
        /// Source database format. Only `sqlite` is supported.
        #[arg(long, default_value = "sqlite")]
        from: String,

        /// Target database format. Only `surrealdb` is supported.
        #[arg(long, default_value = "surrealdb")]
        to: String,

        /// Re-run even if `migration_completed` marker exists.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum PathsAction {
    /// Add a path to index
    Add {
        /// Path to add
        path: String,

        /// Permissions (read, list)
        #[arg(long, default_value = "read,list")]
        permissions: String,
    },

    /// Remove a path from index
    Remove {
        /// Path to remove
        path: String,
    },

    /// List configured paths
    List,
}

/// Ensures the PID file is cleaned up if startup fails or process exits.
struct PidFileGuard;

impl Drop for PidFileGuard {
    fn drop(&mut self) {
        let _ = config::remove_pid();
    }
}

fn main() -> Result<()> {
    // Initialize Rustls crypto provider (required for TLS/WebSocket connections)
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let cli = Cli::parse();

    // Setup logging
    let log_level = if cli.verbose {
        Level::DEBUG
    } else {
        Level::INFO
    };
    let log_path = config::log_path();
    if let Some(parent) = log_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!(
                "Warning: failed to create log directory {}: {}",
                parent.display(),
                e
            );
        }
    }
    let file_log_path = log_path.clone();
    let file_writer = move || -> Box<dyn std::io::Write + Send> {
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_log_path)
        {
            Ok(file) => Box::new(file),
            Err(_) => Box::new(std::io::sink()),
        }
    };
    FmtSubscriber::builder()
        .with_max_level(log_level)
        .with_target(false)
        .with_writer(std::io::stdout.and(file_writer))
        .init();

    // For the `start` command with tray enabled, we must run the native
    // event loop on the OS main thread (macOS requirement). The tokio
    // runtime is created on a background thread instead.
    #[cfg(feature = "tray")]
    if let Commands::Start {
        no_tray: false,
        foreground: false,
    } = &cli.command
    {
        let tray_cfg = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()
            .and_then(|rt| rt.block_on(config::load_config()).ok())
            .unwrap_or_else(|| {
                tracing::warn!("Failed to load config for tray; using defaults");
                config::Config::default()
            });

        return tray::run_with_tray(tray_cfg, move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("Failed to create tokio runtime");
            rt.block_on(async {
                if let Err(e) = cmd_start_services().await {
                    tracing::error!("Agent error: {}", e);
                    std::process::exit(1);
                }
            });
        });
    }

    // All other commands use a normal tokio runtime on the main thread
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    rt.block_on(async {
        match cli.command {
            Commands::Init { hub_url, force } => {
                cmd_init(hub_url, force).await?;
            }
            Commands::Start { .. } => {
                // --no-tray path, or tray feature disabled
                cmd_start_services().await?;
            }
            Commands::Stop => {
                cmd_stop().await?;
            }
            Commands::Status => {
                cmd_status().await?;
            }
            Commands::Config { show, path, set } => {
                cmd_config(show, path, set).await?;
            }
            Commands::Paths { action } => {
                cmd_paths(action).await?;
            }
            Commands::Search { query, limit } => {
                cmd_search(&query, limit).await?;
            }
            Commands::Reindex {
                force,
                quantizer,
                store: store_id,
            } => {
                if let Some(ref qv) = quantizer {
                    let sid = store_id.as_deref().ok_or_else(|| {
                        anyhow::anyhow!("--store is required when --quantizer is specified")
                    })?;
                    cmd_reindex_quantizer(qv, sid).await?;
                } else {
                    cmd_reindex(force).await?;
                }
            }
            Commands::Ui => {
                cmd_ui().await?;
            }
            Commands::Logs { follow, lines } => {
                cmd_logs(follow, lines).await?;
            }
            Commands::Migrate { from, to, force } => {
                if from != "sqlite" {
                    anyhow::bail!("--from only supports 'sqlite' in 1.0.0 (got {})", from);
                }
                if to != "surrealdb" {
                    anyhow::bail!("--to only supports 'surrealdb' in 1.0.0 (got {})", to);
                }
                let sqlite_path = config::sqlite_path();
                let surreal_dir = config::data_dir().join("surreal");
                if !sqlite_path.exists() {
                    anyhow::bail!(
                        "No legacy SQLite DB found at {:?}. Nothing to migrate.",
                        sqlite_path
                    );
                }
                info!(
                    "Starting migration: {:?} → {:?} (force={})",
                    sqlite_path, surreal_dir, force
                );
                let report = migrate::migrate(&sqlite_path, &surreal_dir, force).await?;
                println!(
                    "Migration complete:\n  \
                     users: {}\n  stores: {}\n  articles: {}\n  conversations: {}\n  \
                     messages: {}\n  k2k_clients: {}\n  federation_agreements: {}\n  \
                     discovered_nodes: {}\n  connector_configs: {}",
                    report.users, report.stores, report.articles,
                    report.conversations, report.messages, report.k2k_clients,
                    report.federation_agreements, report.discovered_nodes,
                    report.connector_configs,
                );
            }
        }
        Ok(())
    })
}

async fn cmd_init(hub_url: Option<String>, force: bool) -> Result<()> {
    info!("Initializing Knowledge Nexus Agent...");
    config::init_config(hub_url, force).await
}

/// Open the SurrealDB store, creating the owner user on first run.
/// Refuses to start if a legacy SQLite DB exists but no migration has run.
async fn open_store_or_bail(cfg: &config::Config) -> Result<std::sync::Arc<dyn store::Store>> {
    let surreal_dir = config::data_dir().join("surreal");
    let sqlite_path = config::sqlite_path();
    let surreal_exists = surreal_dir.exists()
        && surreal_dir.read_dir().map(|mut d| d.next().is_some()).unwrap_or(false);
    let migration_complete = migrate::is_migrated(&surreal_dir);
    let legacy_sqlite_exists = sqlite_path.exists();

    match (surreal_exists, migration_complete, legacy_sqlite_exists) {
        (true, true, _) => {
            info!("Opening SurrealDB at {:?}", surreal_dir);
        }
        (true, false, _) => {
            anyhow::bail!(
                "SurrealDB directory {:?} exists but has no `migration_completed` marker. \
                 A previous migration was interrupted. Run: \
                 `knowledge-nexus-agent migrate --force` to retry.",
                surreal_dir
            );
        }
        (false, _, true) => {
            anyhow::bail!(
                "Legacy SQLite DB at {:?} detected, but no SurrealDB yet. Run: \
                 `knowledge-nexus-agent migrate --from sqlite --to surrealdb` to upgrade.",
                sqlite_path
            );
        }
        (false, _, false) => {
            info!("No existing database — creating fresh SurrealDB at {:?}", surreal_dir);
        }
    }

    let surreal_store = store::SurrealStore::open(&surreal_dir).await?;

    if surreal_store.get_owner_user().await?.is_none() {
        let now = chrono::Utc::now().to_rfc3339();
        let user_id = uuid::Uuid::new_v4().to_string();
        let store_id = uuid::Uuid::new_v4().to_string();

        let user = store::User {
            id: user_id.clone(),
            username: cfg.device.name.clone(),
            display_name: cfg.device.name.clone(),
            is_owner: true,
            settings: serde_json::json!({}),
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        surreal_store.create_user(&user).await?;
        info!("Created default owner user: {}", user.username);

        let ks = store::KnowledgeStore {
            id: store_id.clone(),
            owner_id: user_id,
            store_type: "personal".into(),
            name: format!("{}'s Knowledge", cfg.device.name),
            lancedb_collection: format!("store_{}", store_id),
            quantizer_version: "ivf_pq_v1".into(),
            created_at: now.clone(),
            updated_at: now,
        };
        surreal_store.create_store(&ks).await?;
        info!("Created default personal store: {}", ks.name);
    }

    Ok(std::sync::Arc::new(surreal_store))
}

async fn cmd_start_services() -> Result<()> {
    info!("Starting Knowledge Nexus Agent...");

    // Check if already running
    if let Ok(Some(pid)) = config::read_pid() {
        if config::is_process_running(pid) {
            anyhow::bail!("Agent is already running (PID: {})", pid);
        } else {
            // Stale PID file, remove it
            config::remove_pid()?;
        }
    }

    // Pre-flight validation
    let config_file = config::config_path();
    if !config_file.exists() {
        anyhow::bail!(
            "Configuration file not found at {:?}. Run 'knowledge-nexus-agent init' first.",
            config_file
        );
    }

    // Write PID file
    config::write_pid()?;
    let _pid_guard = PidFileGuard;

    // Load configuration
    let cfg = config::load_config().await?;
    info!("Configuration loaded from {:?}", config::config_path());

    // Verify config directory is writable
    let config_dir = config::config_dir();
    if !config_dir.exists() {
        std::fs::create_dir_all(&config_dir)?;
    }

    // Verify data directory is accessible
    let data_dir = config::data_dir();
    if !data_dir.exists() {
        std::fs::create_dir_all(&data_dir)?;
    }

    // Verify ONNX model files exist if K2K is enabled
    if cfg.k2k.enabled {
        let model_dir = data_dir.join("models");
        let model_path = model_dir.join("minilm-l6-v2.onnx");
        let tokenizer_path = model_dir.join("tokenizer.json");
        if !model_path.exists() || !tokenizer_path.exists() {
            info!("ONNX model files not found — they will be downloaded on first use");
        }
    }

    // Open SurrealDB (with migration-detection guard)
    let store = open_store_or_bail(&cfg).await?;

    // Initialize shared resources for K2K server
    let vectordb = if cfg.k2k.enabled {
        info!("Initializing vector database for K2K server...");
        // Resolve quantizer from the default store's quantizer_version
        let quantizer = {
            let registry = vectordb::quantizer::QuantizerRegistry::new();
            let stores = store.list_stores().await?;
            let version = stores
                .first()
                .map(|s| s.quantizer_version.as_str())
                .unwrap_or("ivf_pq_v1");
            registry.resolve(version)?
        };
        Some(std::sync::Arc::new(vectordb::VectorDB::open(quantizer).await?))
    } else {
        None
    };

    let embedding_model = if cfg.k2k.enabled {
        info!("Initializing embedding model for K2K server...");
        Some(embeddings::EmbeddingModel::new()?)
    } else {
        None
    };

    // Spawn each service independently so they don't terminate each other
    // Each service will restart internally if it fails

    // Start the local web UI server
    let ui_handle = if cfg.ui.enabled {
        let ui_cfg = cfg.clone();
        Some(tokio::spawn(async move {
            while let Err(e) = ui::start_server(ui_cfg.clone()).await {
                tracing::error!("UI server error: {}. Restarting in 5s...", e);
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        }))
    } else {
        info!("Local UI disabled in configuration");
        None
    };

    // Create node registry (shared between K2K server and mDNS)
    let node_registry = if cfg.k2k.enabled {
        Some(std::sync::Arc::new(discovery::NodeRegistry::new(
            store.clone(),
        )))
    } else {
        None
    };

    // Start K2K federation server (if enabled) with restart logic
    let k2k_cfg = cfg.clone();
    let k2k_db = store.clone();
    let k2k_handle = if cfg.k2k.enabled {
        let vdb = vectordb.expect("VectorDB should be initialized");
        let emb = embedding_model.expect("EmbeddingModel should be initialized");
        let emb_arc = std::sync::Arc::new(tokio::sync::Mutex::new(emb));

        // Initialize connector registry and register the local file connector
        let connector_registry = std::sync::Arc::new(connectors::ConnectorRegistry::new());
        let local_file_connector = std::sync::Arc::new(connectors::LocalFileConnector::new(
            vdb.clone(),
            emb_arc.clone(),
            cfg.device.id.clone(),
        ));
        connector_registry.register(local_file_connector).await;

        // Only pass the node registry to the K2K server (and thus to RemoteQueryExecutor)
        // when `discovery.auto_federate` is explicitly enabled. Without this gate any
        // mDNS-discovered node on the local network — including spoofed announcements —
        // would receive user query strings with no TLS or identity verification.
        // When auto_federate is false (the default) nodes are still discovered and stored
        // for display, but no federated queries are sent to them.
        let k2k_node_registry = if cfg.discovery.auto_federate {
            if node_registry.is_some() {
                tracing::warn!(
                    "Remote federation is enabled (discovery.auto_federate=true). \
                     mDNS-discovered nodes will receive query data. Only enable this \
                     on a fully trusted network."
                );
            }
            node_registry.clone()
        } else {
            None
        };

        Some(tokio::spawn(async move {
            loop {
                let config_dir = config::config_dir();
                match k2k::K2KServer::new(
                    k2k_cfg.clone(),
                    config_dir,
                    vdb.clone(),
                    emb_arc.clone(),
                    k2k_db.clone(),
                    connector_registry.clone(),
                    k2k_node_registry.clone(),
                )
                .await
                {
                    Ok(server) => {
                        if let Err(e) = server.start().await {
                            tracing::error!("K2K server error: {}, restarting in 5s", e);
                            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                        } else {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::error!("K2K server init failed: {}, retrying in 10s", e);
                        tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                    }
                }
            }
        }))
    } else {
        info!("K2K server disabled in configuration");
        None
    };

    // Start mDNS discovery (if enabled) with restart logic
    let mdns_handle = if cfg.discovery.enabled && cfg.k2k.enabled {
        let mdns_node_registry = node_registry
            .clone()
            .expect("Node registry should be initialized");
        let mdns_cfg = cfg.clone();

        Some(tokio::spawn(async move {
            loop {
                let registry = mdns_node_registry.clone();
                match discovery::MDNSService::new(mdns_cfg.clone(), registry) {
                    Ok(mdns) => {
                        if let Err(e) = mdns.advertise() {
                            tracing::error!("Failed to advertise via mDNS: {}, retrying in 10s", e);
                            tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                            continue;
                        }
                        if let Err(e) = mdns.browse_loop().await {
                            tracing::error!("mDNS browse error: {}, restarting in 5s", e);
                            tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                        } else {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::error!("mDNS service init failed: {}, retrying in 10s", e);
                        tokio::time::sleep(tokio::time::Duration::from_secs(10)).await;
                    }
                }
            }
        }))
    } else {
        if !cfg.discovery.enabled {
            info!("mDNS discovery disabled in configuration");
        }
        None
    };

    // Start WebSocket connection to hub (if configured)
    let conn_cfg = cfg.clone();
    let conn_handle = tokio::spawn(async move {
        // Connection module handles its own reconnection logic
        if let Err(e) = connection::start_connection(conn_cfg).await {
            tracing::error!("Connection error: {}", e);
        }
    });

    // Start file watcher for indexing
    let watcher_handle = if cfg.indexing.watch_enabled {
        let watcher_cfg = cfg.clone();
        Some(tokio::spawn(async move {
            while let Err(e) = search::start_watcher(watcher_cfg.clone()).await {
                tracing::error!("File watcher error: {}. Restarting in 5s...", e);
                tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
            }
        }))
    } else {
        info!("File watcher disabled in configuration");
        None
    };

    info!("All services started. Press Ctrl+C to stop.");

    // Wait for shutdown signal
    tokio::signal::ctrl_c().await?;
    info!("Received shutdown signal, stopping services...");

    // Abort all spawned tasks
    if let Some(handle) = ui_handle {
        handle.abort();
    }
    if let Some(handle) = k2k_handle {
        handle.abort();
    }
    if let Some(handle) = mdns_handle {
        handle.abort();
    }
    conn_handle.abort();
    if let Some(handle) = watcher_handle {
        handle.abort();
    }

    info!("Agent stopped");
    Ok(())
}

async fn cmd_stop() -> Result<()> {
    info!("Stopping Knowledge Nexus Agent...");

    match config::read_pid()? {
        Some(pid) => {
            if config::is_process_running(pid) {
                if config::stop_process(pid)? {
                    println!("Agent stopped (PID: {})", pid);
                } else {
                    println!("Failed to stop agent (PID: {})", pid);
                }
            } else {
                // Stale PID file
                config::remove_pid()?;
                println!("Agent was not running (cleaned up stale PID file)");
            }
        }
        None => {
            println!("Agent is not running (no PID file found)");
        }
    }

    Ok(())
}

async fn cmd_status() -> Result<()> {
    let cfg = config::load_config().await?;

    // Check if agent is running
    let (status, pid) = match config::read_pid()? {
        Some(pid) => {
            if config::is_process_running(pid) {
                ("Running", Some(pid))
            } else {
                ("Not running (stale PID file)", None)
            }
        }
        None => ("Not running", None),
    };

    println!("Knowledge Nexus System Agent");
    println!("============================");
    println!();
    print!("Status: {}", status);
    if let Some(p) = pid {
        println!(" (PID: {})", p);
    } else {
        println!();
    }
    println!("Device: {} ({})", cfg.device.name, cfg.device.id);
    println!();
    println!(
        "Hub: {}",
        cfg.connection
            .hub_url
            .as_deref()
            .unwrap_or("Not configured")
    );

    // Check hub connection status by checking if local UI is responding
    let connection_status = if pid.is_some() {
        if cfg.connection.hub_url.is_some()
            && cfg
                .connection
                .auth_token
                .as_deref()
                .map(|t| !t.trim().is_empty())
                .unwrap_or(false)
        {
            "Configured (check logs for status)"
        } else if cfg.connection.hub_url.is_some() {
            "Hub URL configured, but auth token is missing"
        } else {
            "Offline mode (no hub configured)"
        }
    } else {
        "Not connected (agent not running)"
    };
    println!("Connection: {}", connection_status);
    println!();
    println!("Indexed Paths:");
    for path in &cfg.security.allowed_paths {
        println!("  - {}", path);
    }
    println!();
    if cfg.ui.enabled {
        println!("Local UI: http://localhost:{}", cfg.ui.port);
    } else {
        println!("Local UI: disabled");
    }
    println!("Config file: {}", config::config_path().display());
    println!("Data directory: {}", config::data_dir().display());

    Ok(())
}

/// Recursively redact known sensitive fields in a JSON value so they are
/// never printed to stdout by `config show`.
fn redact_sensitive_fields(value: &mut serde_json::Value) {
    const SENSITIVE_KEYS: &[&str] = &["auth_token", "api_key", "registration_secret", "password", "secret"];
    match value {
        serde_json::Value::Object(map) => {
            for (key, val) in map.iter_mut() {
                let key_lower = key.to_lowercase();
                if SENSITIVE_KEYS.iter().any(|s| key_lower.contains(s)) {
                    if !val.is_null() {
                        *val = serde_json::Value::String("[REDACTED]".to_string());
                    }
                } else {
                    redact_sensitive_fields(val);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr.iter_mut() {
                redact_sensitive_fields(v);
            }
        }
        _ => {}
    }
}

async fn cmd_config(show: bool, path: bool, set: Option<String>) -> Result<()> {
    if path {
        println!("{}", config::config_path().display());
        return Ok(());
    }

    if show {
        let cfg = config::load_config().await?;
        let mut json_val = serde_json::to_value(&cfg)?;
        redact_sensitive_fields(&mut json_val);
        println!("{}", serde_json::to_string_pretty(&json_val)?);
        return Ok(());
    }

    if let Some(kv) = set {
        // Parse KEY=VALUE and update config
        let parts: Vec<&str> = kv.splitn(2, '=').collect();
        if parts.len() != 2 {
            anyhow::bail!("Invalid format. Use KEY=VALUE (e.g., device.name=MyMachine)");
        }

        let key = parts[0].trim();
        let value = parts[1].trim();

        let mut cfg = config::load_config().await?;

        // Handle known configuration keys
        match key {
            "device.name" => cfg.device.name = value.to_string(),
            "device.id" => cfg.device.id = value.to_string(),
            "connection.hub_url" => cfg.connection.hub_url = Some(value.to_string()),
            "connection.auth_token" => cfg.connection.auth_token = Some(value.to_string()),
            "connection.heartbeat_interval" => cfg.connection.heartbeat_interval = value.parse()?,
            "ui.port" => cfg.ui.port = value.parse()?,
            "ui.enabled" => cfg.ui.enabled = value.parse()?,
            "indexing.watch_enabled" => cfg.indexing.watch_enabled = value.parse()?,
            "indexing.debounce_ms" => cfg.indexing.debounce_ms = value.parse()?,
            "indexing.max_file_size" => cfg.indexing.max_file_size = value.parse()?,
            "security.max_file_size" => cfg.security.max_file_size = value.parse()?,
            "security.allow_read" => cfg.security.allow_read = value.parse()?,
            "security.allow_write" => cfg.security.allow_write = value.parse()?,
            _ => anyhow::bail!("Unknown configuration key: {}\n\nAvailable keys:\n  device.name, device.id\n  connection.hub_url, connection.auth_token, connection.heartbeat_interval\n  ui.port, ui.enabled\n  indexing.watch_enabled, indexing.debounce_ms, indexing.max_file_size\n  security.max_file_size, security.allow_read, security.allow_write", key),
        }

        config::save_config(&cfg).await?;
        const CREDENTIAL_KEYS: &[&str] = &["auth_token", "api_key", "registration_secret", "password", "secret"];
        let display_value = if CREDENTIAL_KEYS.iter().any(|k| key.to_lowercase().contains(k)) { "[REDACTED]" } else { value };
        println!("Updated {} = {}", key, display_value);
        return Ok(());
    }

    // Open config in editor
    let path = config::config_path();
    println!("Config file: {}", path.display());
    #[cfg(windows)]
    println!("Edit with: notepad {}", path.display());
    #[cfg(not(windows))]
    println!("Edit with: $EDITOR {}", path.display());

    Ok(())
}

async fn cmd_paths(action: PathsAction) -> Result<()> {
    match action {
        PathsAction::Add { path, permissions } => {
            if permissions.trim() != "read,list" {
                tracing::warn!(
                    "Path permissions are currently informational only; using default read/list behavior"
                );
            }
            // Expand path (handles ~ on all platforms)
            let expanded = shellexpand::tilde(&path).to_string();

            // Validate path exists
            let path_buf = std::path::PathBuf::from(&expanded);
            if !path_buf.exists() {
                anyhow::bail!("Path does not exist: {}", expanded);
            }

            let mut cfg = config::load_config().await?;

            // Check if already added
            if cfg.security.allowed_paths.contains(&expanded) {
                println!("Path already configured: {}", expanded);
                return Ok(());
            }

            cfg.security.allowed_paths.push(expanded.clone());
            config::save_config(&cfg).await?;
            println!("Added path: {}", expanded);
            println!("Run 'knowledge-nexus-agent reindex' to index files in this path");
        }
        PathsAction::Remove { path } => {
            let mut cfg = config::load_config().await?;

            // Expand path for comparison
            let expanded = if path.starts_with('~') {
                dirs::home_dir()
                    .map(|home| path.replacen('~', &home.to_string_lossy(), 1))
                    .unwrap_or(path.clone())
            } else {
                path.clone()
            };

            let original_len = cfg.security.allowed_paths.len();
            cfg.security
                .allowed_paths
                .retain(|p| p != &expanded && p != &path);

            if cfg.security.allowed_paths.len() == original_len {
                println!("Path not found in configuration: {}", path);
                return Ok(());
            }

            config::save_config(&cfg).await?;
            println!("Removed path: {}", expanded);
        }
        PathsAction::List => {
            let cfg = config::load_config().await?;
            println!("Configured paths:");
            if cfg.security.allowed_paths.is_empty() {
                println!("  (none)");
            } else {
                for path in &cfg.security.allowed_paths {
                    let exists = std::path::Path::new(path).exists();
                    let status = if exists { "" } else { " (missing)" };
                    println!("  - {}{}", path, status);
                }
            }
        }
    }
    Ok(())
}

async fn cmd_search(query: &str, limit: usize) -> Result<()> {
    info!("Searching for: {}", query);

    let cfg = config::load_config().await?;
    let results = search::search_files(&cfg, query, limit).await?;

    if results.is_empty() {
        println!("No results found for: {}", query);
        return Ok(());
    }

    println!("Found {} results:\n", results.len());
    for (i, result) in results.iter().enumerate() {
        println!("{}. {} (score: {:.2})", i + 1, result.path, result.score);
        if let Some(snippet) = &result.snippet {
            println!("   {}", snippet);
        }
        println!();
    }

    Ok(())
}

async fn cmd_reindex_quantizer(quantizer_version: &str, store_id: &str) -> Result<()> {
    let registry = vectordb::quantizer::QuantizerRegistry::new();
    let quantizer = registry.resolve(quantizer_version)?;

    info!(
        "Rebuilding LanceDB index for store '{}' with quantizer '{}'",
        store_id, quantizer_version
    );

    let surreal_dir = config::data_dir().join("surreal");
    let surreal_exists = surreal_dir.exists()
        && surreal_dir
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(false);
    let migration_complete = migrate::is_migrated(&surreal_dir);

    if !surreal_exists || !migration_complete {
        anyhow::bail!(
            "SurrealDB is not ready at {:?}. Run `knowledge-nexus-agent migrate` first.",
            surreal_dir
        );
    }

    let db = store::SurrealStore::open(&surreal_dir).await?;

    let store_record = db
        .get_store(store_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Store '{}' not found", store_id))?;

    println!(
        "Store '{}': switching quantizer from '{}' to '{}'",
        store_record.name, store_record.quantizer_version, quantizer_version
    );

    let vectordb = vectordb::VectorDB::open(quantizer).await?;
    vectordb.build_index().await?;

    db.update_store_quantizer_version(store_id, quantizer_version)
        .await?;

    println!(
        "Reindex complete. Store now uses quantizer '{}'.",
        quantizer_version
    );
    Ok(())
}

async fn cmd_reindex(force: bool) -> Result<()> {
    info!("Reindexing files (force={})", force);
    let cfg = config::load_config().await?;
    search::reindex_all(&cfg, force).await?;
    println!("Reindex complete");
    Ok(())
}

async fn cmd_ui() -> Result<()> {
    let cfg = config::load_config().await?;
    if !cfg.ui.enabled {
        anyhow::bail!("Local UI is disabled in configuration (ui.enabled=false)");
    }
    let url = format!("http://localhost:{}", cfg.ui.port);
    println!("Opening: {}", url);

    // Try to open in browser
    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open").arg(&url).spawn()?;

    #[cfg(target_os = "macos")]
    std::process::Command::new("open").arg(&url).spawn()?;

    #[cfg(target_os = "windows")]
    std::process::Command::new("cmd")
        .args(["/C", "start", &url])
        .spawn()?;

    Ok(())
}

async fn cmd_logs(follow: bool, lines: usize) -> Result<()> {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};

    let log_path = config::log_path();

    if !log_path.exists() {
        println!("No log file found at: {}", log_path.display());
        println!("Start the agent first: knowledge-nexus-agent start");
        return Ok(());
    }

    println!("Log file: {}", log_path.display());
    println!();

    // Read last N lines
    let file = std::fs::File::open(&log_path)?;
    let reader = BufReader::new(&file);
    let all_lines: Vec<String> = reader.lines().map_while(Result::ok).collect();

    let start = if all_lines.len() > lines {
        all_lines.len() - lines
    } else {
        0
    };

    for line in &all_lines[start..] {
        println!("{}", line);
    }

    if follow {
        println!();
        println!("--- Following logs (Ctrl+C to stop) ---");

        // Tail the file
        let mut file = std::fs::File::open(&log_path)?;
        file.seek(SeekFrom::End(0))?;
        let mut reader = BufReader::new(file);

        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    // No new data, sleep briefly
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                Ok(_) => {
                    print!("{}", line);
                }
                Err(e) => {
                    tracing::error!("Error reading log: {}", e);
                    break;
                }
            }
        }
    }

    Ok(())
}
