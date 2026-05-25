//! Configuration management for the System Agent.
//!
//! Configuration is stored in YAML format at platform-specific locations:
//! - Linux: ~/.config/knowledge-nexus-agent/config.yaml
//! - macOS: ~/Library/Application Support/knowledge-nexus-agent/config.yaml
//! - Windows: %APPDATA%/knowledge-nexus-agent/config.yaml

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;
use uuid::Uuid;

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Device identification
    pub device: DeviceConfig,

    /// Connection settings
    pub connection: ConnectionConfig,

    /// Security settings (locally enforced)
    pub security: SecurityConfig,

    /// Local UI settings
    pub ui: UiConfig,

    /// Embedding/indexing settings
    pub indexing: IndexingConfig,

    /// K2K federation server settings
    #[serde(default)]
    pub k2k: K2KConfig,

    /// Micro-backend settings (standalone mode)
    #[serde(default)]
    pub micro_backend: MicroBackendConfig,

    /// mDNS discovery settings
    #[serde(default)]
    pub discovery: DiscoveryConfig,

    /// Web search capability settings
    #[serde(default)]
    pub web_search: WebSearchConfig,

    /// Entity extraction settings (P3)
    #[serde(default)]
    pub extraction: ExtractionConfig,

    /// Retrieval pipeline settings (P4)
    #[serde(default)]
    pub retrieval: RetrievalConfig,

    /// Graph / multi-graph settings (P5)
    #[serde(default)]
    pub graph: GraphConfig,

    /// Reflection scheduling settings (P7)
    #[serde(default)]
    pub reflection: ReflectionConfig,

    /// Decay / tier-transition settings (P8)
    #[serde(default)]
    pub decay: DecayConfig,

    /// Compaction settings (P8)
    #[serde(default)]
    pub compaction: CompactionConfig,

    /// Agent-native HTTP API settings (P9)
    #[serde(default)]
    pub agent_api: AgentApiConfig,

    /// Policy-learned memory control settings (P10)
    #[serde(default)]
    pub policy: PolicyConfig,
}

// ============================================================================
// Capability mesh configuration (service-mesh extension points)
// ============================================================================

/// How a configured capability is implemented
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityHandlerType {
    /// Handled locally by a built-in TaskHandler
    Local,
    /// Delegated to a remote HTTP endpoint
    Remote,
}

impl Default for CapabilityHandlerType {
    fn default() -> Self {
        Self::Local
    }
}

/// Authentication configuration for remote capability handlers
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum RemoteAuthConfig {
    None,
    BearerToken { token: String },
    ApiKey { header: String, value: String },
}

impl std::fmt::Debug for RemoteAuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "None"),
            Self::BearerToken { .. } => f.debug_struct("BearerToken").field("token", &"[REDACTED]").finish(),
            Self::ApiKey { header, .. } => f.debug_struct("ApiKey").field("header", header).field("value", &"[REDACTED]").finish(),
        }
    }
}

impl Default for RemoteAuthConfig {
    fn default() -> Self {
        Self::None
    }
}

/// A single capability entry in the capabilities config file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityEntry {
    /// Unique ID (matches capability_id used in task requests)
    pub id: String,
    /// Human-readable name
    pub name: String,
    /// Category: Knowledge, Tool, Research, Communication, Storage, etc.
    pub category: String,
    /// Human-readable description
    pub description: String,
    /// JSON Schema describing accepted input (optional)
    #[serde(default)]
    pub input_schema: Option<serde_json::Value>,
    /// JSON Schema describing output (optional)
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
    /// Whether this is handled locally or delegated to a remote service
    #[serde(default)]
    pub handler_type: CapabilityHandlerType,
    /// For remote handlers: the HTTP endpoint URL
    #[serde(default)]
    pub endpoint_url: Option<String>,
    /// For remote handlers: HTTP method (default POST)
    #[serde(default = "default_http_method")]
    pub http_method: String,
    /// For remote handlers: authentication config
    #[serde(default)]
    pub auth: RemoteAuthConfig,
    /// Timeout in seconds for remote calls (default 30)
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// Optional health-check URL (GET) to test availability
    #[serde(default)]
    pub health_check_url: Option<String>,
    /// Optional field name remapping for input (from_field → to_field)
    #[serde(default)]
    pub input_mapping: Option<std::collections::HashMap<String, String>>,
    /// Optional field name remapping for output (from_field → to_field)
    #[serde(default)]
    pub output_mapping: Option<std::collections::HashMap<String, String>>,
    /// Rate limit: max requests per minute (0 = unlimited)
    #[serde(default)]
    pub rate_limit_rpm: u32,
    /// Whether the endpoint supports streaming results
    #[serde(default)]
    pub supports_streaming: bool,
}

fn default_http_method() -> String {
    "POST".to_string()
}

fn default_timeout_secs() -> u64 {
    30
}

/// Capability mesh configuration (stored in capabilities.yaml alongside config.yaml)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CapabilityMeshConfig {
    /// List of capability entries
    #[serde(default)]
    pub capabilities: Vec<CapabilityEntry>,
    /// Auto-discovery settings
    #[serde(default)]
    pub discovery: ServiceDiscoveryConfig,
}

/// Auto-discovery settings for knowledge-nexus microservices
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceDiscoveryConfig {
    /// Enable auto-discovery via /.well-known/k2k-manifest
    #[serde(default)]
    pub enabled: bool,
    /// Base URLs of knowledge-nexus microservices to probe
    #[serde(default)]
    pub service_base_urls: Vec<String>,
    /// TTL in minutes before re-discovering (default 15)
    #[serde(default = "default_discovery_ttl_minutes")]
    pub ttl_minutes: u64,
}

fn default_discovery_ttl_minutes() -> u64 {
    15
}

impl Default for ServiceDiscoveryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            service_base_urls: vec![],
            ttl_minutes: default_discovery_ttl_minutes(),
        }
    }
}

/// Web search capability configuration
#[derive(Clone, Serialize, Deserialize)]
pub struct WebSearchConfig {
    /// Enable web search capability
    #[serde(default)]
    pub enabled: bool,

    /// Search provider: "brave" | "google" | "bing"
    #[serde(default = "default_search_provider")]
    pub provider: String,

    /// API key for the search provider (can also be set via BRAVE_SEARCH_API_KEY,
    /// GOOGLE_SEARCH_API_KEY, or BING_SEARCH_API_KEY environment variables)
    #[serde(default)]
    pub api_key: Option<String>,

    /// Maximum number of results to return
    #[serde(default = "default_max_search_results")]
    pub max_results: usize,
}

fn default_search_provider() -> String {
    "brave".to_string()
}

fn default_max_search_results() -> usize {
    10
}

impl std::fmt::Debug for WebSearchConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSearchConfig")
            .field("enabled", &self.enabled)
            .field("provider", &self.provider)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("max_results", &self.max_results)
            .finish()
    }
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: default_search_provider(),
            api_key: None,
            max_results: default_max_search_results(),
        }
    }
}

/// Entity extraction configuration (P3)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionConfig {
    /// Enable LLM-based entity extraction during article ingest
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Ollama API base URL
    #[serde(default = "default_ollama_url")]
    pub ollama_url: String,

    /// Ollama model name for entity extraction
    #[serde(default = "default_extraction_model")]
    pub model: String,
}

fn default_ollama_url() -> String {
    "http://localhost:11434".to_string()
}

fn default_extraction_model() -> String {
    "gemma4:e4b".to_string()
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ollama_url: default_ollama_url(),
            model: default_extraction_model(),
        }
    }
}

/// Retrieval pipeline configuration (P4)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalConfig {
    /// RRF constant (higher = more weight to top ranks)
    #[serde(default = "default_rrf_k")]
    pub rrf_k: f32,

    /// Weight for vector search signal in RRF
    #[serde(default = "default_vector_weight")]
    pub vector_weight: f32,

    /// Weight for keyword/FTS search signal in RRF
    #[serde(default = "default_keyword_weight")]
    pub keyword_weight: f32,

    /// Maximum weight for graph search signal (scaled by entity coverage)
    #[serde(default = "default_graph_weight_max")]
    pub graph_weight_max: f32,

    /// Max hops for entity_overlap traversal (1 = direct neighbors only)
    #[serde(default = "default_graph_hops")]
    pub graph_hops: usize,

    /// Which edge types graph traversal should follow. P6 spreading activation
    /// uses richer filters; P4/P5 default is entity_overlap only.
    #[serde(default)]
    pub edge_types: EdgeTypeFilter,

    /// Graph signal strategy: "jaccard" preserves P4 behavior (one-hop
    /// expansion via ENTITY_OVERLAP); "activation" enables P6 spreading
    /// activation via PPR + SYNAPSE post-processing.
    #[serde(default = "default_graph_strategy")]
    pub graph_strategy: String,

    /// Spreading-activation parameters (P6).
    #[serde(default)]
    pub activation: ActivationConfig,

    /// If true, include Archive-tier items in retrieval (default false: P8
    /// archived items are excluded from default queries to honor the
    /// quarantine semantics — surface only via explicit query parameter).
    #[serde(default)]
    pub include_archive: bool,
}

fn default_rrf_k() -> f32 {
    60.0
}
fn default_vector_weight() -> f32 {
    1.0
}
fn default_keyword_weight() -> f32 {
    1.1
}
fn default_graph_weight_max() -> f32 {
    1.0
}
fn default_graph_hops() -> usize {
    1
}
fn default_graph_strategy() -> String {
    "activation".into()
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            rrf_k: default_rrf_k(),
            vector_weight: default_vector_weight(),
            keyword_weight: default_keyword_weight(),
            graph_weight_max: default_graph_weight_max(),
            graph_hops: default_graph_hops(),
            edge_types: EdgeTypeFilter::default(),
            graph_strategy: default_graph_strategy(),
            activation: ActivationConfig::default(),
            include_archive: false,
        }
    }
}

/// Reflection scheduling configuration (P7).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionConfig {
    /// Submit a reflection job for a store after this many ingests.
    /// Set to 0 to disable rate-triggered reflection (manual + scheduled only).
    #[serde(default = "default_ingests_per_reflection")]
    pub ingests_per_reflection_trigger: usize,

    /// Minimum interval between reflection runs per store. The scheduler
    /// dedups within this window via the idempotency key
    /// `reflection:<store_id>:<date>`.
    #[serde(default = "default_min_reflection_interval_hours")]
    pub min_reflection_interval_hours: u64,
}

fn default_ingests_per_reflection() -> usize { 100 }
fn default_min_reflection_interval_hours() -> u64 { 6 }

impl Default for ReflectionConfig {
    fn default() -> Self {
        Self {
            ingests_per_reflection_trigger: default_ingests_per_reflection(),
            min_reflection_interval_hours: default_min_reflection_interval_hours(),
        }
    }
}

/// Choice of salience formula used by the tier-transition job (P8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SalienceFormula {
    /// SYNAPSE-aligned activation-driven decay (default).
    /// `salience = importance · exp(-lambda · days_since_access)`.
    /// Per SYNAPSE ablation: removing decay drops Temporal F1 50.1→14.2.
    ActivationDriven,
    /// MemoryOS heat formula (arXiv 2506.06326).
    /// `heat = α·visits + β·interaction_length + γ·recency_factor` with
    /// recency decay constant μ.
    MemoryOsHeat,
    /// Generative Agents formula (Park et al., named in Du survey 2603.07670).
    /// `salience = w_rec·recency + w_rel·relevance + w_imp·importance`.
    GenerativeAgents,
    /// MemoryBank Ebbinghaus-curve (Du survey 2603.07670 §9.8).
    /// `salience = exp(-t/S)` with reinforcement-on-access raising S.
    Ebbinghaus,
}

impl Default for SalienceFormula {
    fn default() -> Self {
        SalienceFormula::ActivationDriven
    }
}

/// Decay / tier-transition configuration (P8).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecayConfig {
    /// Salience formula choice. Default ActivationDriven (SYNAPSE-aligned).
    #[serde(default)]
    pub formula: SalienceFormula,

    /// Activation-driven decay constant. Default 0.02 ≈ 35-day half-life.
    /// Equivalent: λ in `salience = importance · exp(-λ · days_since_access)`.
    #[serde(default = "default_lambda")]
    pub lambda: f64,

    /// Tier threshold: salience >= this stays in Hot.
    #[serde(default = "default_hot_threshold")]
    pub hot_threshold: f64,
    /// Tier threshold: salience >= this stays in Warm (else Cold).
    #[serde(default = "default_warm_threshold")]
    pub warm_threshold: f64,
    /// Tier threshold: salience >= this stays in Cold (else Archive).
    #[serde(default = "default_cold_threshold")]
    pub cold_threshold: f64,

    // MemoryOS heat formula parameters
    #[serde(default = "default_heat_alpha")]
    pub heat_alpha: f64,
    #[serde(default = "default_heat_beta")]
    pub heat_beta: f64,
    #[serde(default = "default_heat_gamma")]
    pub heat_gamma: f64,
    /// Recency decay constant in MemoryOS heat formula.
    #[serde(default = "default_heat_mu")]
    pub heat_mu: f64,

    // Generative Agents weights
    #[serde(default = "default_ga_w_recency")]
    pub ga_w_recency: f64,
    #[serde(default = "default_ga_w_relevance")]
    pub ga_w_relevance: f64,
    #[serde(default = "default_ga_w_importance")]
    pub ga_w_importance: f64,
    /// Recency decay constant for the Generative Agents formula.
    #[serde(default = "default_ga_decay")]
    pub ga_decay: f64,

    // Ebbinghaus
    /// Memory strength constant in `salience = exp(-t/S)`. Higher = slower decay.
    #[serde(default = "default_ebbinghaus_strength")]
    pub ebbinghaus_strength: f64,
}

fn default_lambda() -> f64 { 0.02 }
fn default_hot_threshold() -> f64 { 0.5 }
fn default_warm_threshold() -> f64 { 0.1 }
fn default_cold_threshold() -> f64 { 0.01 }

fn default_heat_alpha() -> f64 { 1.0 }
fn default_heat_beta() -> f64 { 0.5 }
fn default_heat_gamma() -> f64 { 2.0 }
fn default_heat_mu() -> f64 { 1.0e7 }

fn default_ga_w_recency() -> f64 { 0.5 }
fn default_ga_w_relevance() -> f64 { 0.3 }
fn default_ga_w_importance() -> f64 { 0.2 }
fn default_ga_decay() -> f64 { 0.99 }

fn default_ebbinghaus_strength() -> f64 { 10.0 }

impl Default for DecayConfig {
    fn default() -> Self {
        Self {
            formula: SalienceFormula::default(),
            lambda: default_lambda(),
            hot_threshold: default_hot_threshold(),
            warm_threshold: default_warm_threshold(),
            cold_threshold: default_cold_threshold(),
            heat_alpha: default_heat_alpha(),
            heat_beta: default_heat_beta(),
            heat_gamma: default_heat_gamma(),
            heat_mu: default_heat_mu(),
            ga_w_recency: default_ga_w_recency(),
            ga_w_relevance: default_ga_w_relevance(),
            ga_w_importance: default_ga_w_importance(),
            ga_decay: default_ga_decay(),
            ebbinghaus_strength: default_ebbinghaus_strength(),
        }
    }
}

/// Agent-native HTTP API configuration (P9).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentApiConfig {
    /// Master switch. Default true — the /v1/memory/* endpoints are
    /// mounted on the existing K2K server.
    #[serde(default = "default_agent_api_enabled")]
    pub enabled: bool,

    /// Default token budget for recall when caller doesn't specify.
    #[serde(default = "default_token_budget")]
    pub default_token_budget: u32,

    /// Maximum token budget callers can request.
    #[serde(default = "default_max_token_budget")]
    pub max_token_budget: u32,

    /// p95 latency target for recall (informational; enforced via per-query
    /// deadline). Used by smoke tests + ops dashboards.
    #[serde(default = "default_recall_p95_ms")]
    pub recall_p95_target_ms: u64,

    /// Allow federation fan-out on /v1/memory/recall when caller sets
    /// federate=true. Default false (explicit opt-in even though P9 supports it).
    #[serde(default)]
    pub federation_enabled: bool,
}

fn default_agent_api_enabled() -> bool { true }
fn default_token_budget() -> u32 { 2000 }
fn default_max_token_budget() -> u32 { 16_000 }
fn default_recall_p95_ms() -> u64 { 1500 }

impl Default for AgentApiConfig {
    fn default() -> Self {
        Self {
            enabled: default_agent_api_enabled(),
            default_token_budget: default_token_budget(),
            max_token_budget: default_max_token_budget(),
            recall_p95_target_ms: default_recall_p95_ms(),
            federation_enabled: false,
        }
    }
}

/// Policy-learned memory control configuration (P10 scaffolding).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConfig {
    /// Name of the active DecayPolicy impl. P10 scaffolding: only
    /// "default_synapse_aligned" is implemented. Future learned impls
    /// register themselves here.
    #[serde(default = "default_decay_policy_name")]
    pub decay_policy_name: String,

    /// Name of the active ReflectionTriggerPolicy impl.
    #[serde(default = "default_reflection_policy_name")]
    pub reflection_policy_name: String,

    /// Name of the active ActivationWeightPolicy impl.
    #[serde(default = "default_activation_policy_name")]
    pub activation_policy_name: String,

    /// Fraction of policy decisions to log as traces (0.0 = none,
    /// 1.0 = all). Lower in production to manage table volume; keep
    /// at 1.0 during dev to ensure training-data accumulates.
    #[serde(default = "default_trace_sampling_rate")]
    pub trace_sampling_rate: f64,
}

fn default_decay_policy_name() -> String { "default_synapse_aligned".into() }
fn default_reflection_policy_name() -> String { "default_rate_threshold".into() }
fn default_activation_policy_name() -> String { "default_magma_table".into() }
fn default_trace_sampling_rate() -> f64 { 1.0 }

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            decay_policy_name: default_decay_policy_name(),
            reflection_policy_name: default_reflection_policy_name(),
            activation_policy_name: default_activation_policy_name(),
            trace_sampling_rate: default_trace_sampling_rate(),
        }
    }
}

/// Compaction-via-reflection configuration (P8).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    /// Minimum cluster size (number of Cold-tier articles sharing an entity)
    /// to qualify for compaction. Smaller clusters are skipped.
    #[serde(default = "default_compaction_min_cluster")]
    pub min_cluster_size: usize,

    /// Maximum number of clusters to compact per run (rate-limiting).
    #[serde(default = "default_compaction_max_clusters_per_run")]
    pub max_clusters_per_run: usize,
}

fn default_compaction_min_cluster() -> usize { 5 }
fn default_compaction_max_clusters_per_run() -> usize { 50 }

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            min_cluster_size: default_compaction_min_cluster(),
            max_clusters_per_run: default_compaction_max_clusters_per_run(),
        }
    }
}

/// Graph / multi-graph configuration (P5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphConfig {
    /// Cosine similarity threshold for SEMANTICALLY_RELATED edges
    /// (HippoRAG uses 0.8, SYNAPSE 0.92; KNL midpoint default).
    #[serde(default = "default_semantic_threshold")]
    pub semantic_threshold: f64,

    /// Top-K nearest neighbors to scan per article during semantic backfill.
    #[serde(default = "default_semantic_top_k")]
    pub semantic_top_k: usize,

    /// If true, attempt causal extraction during ingestion (LLM-bound).
    /// Default false: opt-in because expensive and there is no honest heuristic.
    #[serde(default)]
    pub causal_enabled: bool,

    /// Ollama API base URL for causal extraction.
    #[serde(default = "default_ollama_url")]
    pub ollama_url: String,

    /// Ollama model for causal extraction. Independent of `extraction.model`
    /// because causal extraction is sensitive to instruction-following
    /// (typically wants a larger model than entity extraction).
    #[serde(default = "default_causal_model")]
    pub causal_model: String,

    /// Confidence threshold below which causal edges are stored for audit
    /// only and excluded from retrieval traversal.
    #[serde(default = "default_causal_confidence_threshold")]
    pub causal_confidence_threshold: f64,
}

fn default_semantic_threshold() -> f64 { 0.85 }
fn default_semantic_top_k() -> usize { 20 }
fn default_causal_model() -> String { "llama3.2:3b".into() }
fn default_causal_confidence_threshold() -> f64 { 0.6 }

impl Default for GraphConfig {
    fn default() -> Self {
        Self {
            semantic_threshold: default_semantic_threshold(),
            semantic_top_k: default_semantic_top_k(),
            causal_enabled: false,
            ollama_url: default_ollama_url(),
            causal_model: default_causal_model(),
            causal_confidence_threshold: default_causal_confidence_threshold(),
        }
    }
}

/// Which edge types graph traversal should follow. Defaults preserve P4
/// behavior (entity_overlap only). P6 spreading activation uses richer filters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeTypeFilter {
    #[serde(default = "default_true")]
    pub entity_overlap: bool,
    #[serde(default)]
    pub semantically_related: bool,
    #[serde(default)]
    pub precedes: bool,
    #[serde(default)]
    pub caused_by: bool,
    #[serde(default)]
    pub references: bool,
}

impl Default for EdgeTypeFilter {
    fn default() -> Self {
        Self {
            entity_overlap: true,
            semantically_related: false,
            precedes: false,
            caused_by: false,
            references: false,
        }
    }
}

/// Spreading-activation configuration (P6).
///
/// Defaults are from peer-reviewed sources:
/// - HippoRAG (arXiv 2405.14831): damping=0.5, tolerance=1e-4
/// - SYNAPSE (arXiv 2601.02744): β=0.15, M=7, γ=5.0, τ=0.12, |V|≤10k
/// - MAGMA (arXiv 2601.03236): per-intent edge weight multipliers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivationConfig {
    /// PPR damping factor (HippoRAG default).
    #[serde(default = "default_ppr_damping")]
    pub damping: f32,

    /// PPR convergence tolerance.
    #[serde(default = "default_ppr_tolerance")]
    pub tolerance: f32,

    /// Maximum PPR iterations before forced termination.
    #[serde(default = "default_ppr_max_iter")]
    pub max_iter: usize,

    /// Active subgraph cap (max nodes considered per query, SYNAPSE).
    #[serde(default = "default_subgraph_cap")]
    pub subgraph_cap: usize,

    /// SYNAPSE lateral inhibition strength.
    #[serde(default = "default_inhibition_beta")]
    pub inhibition_beta: f32,

    /// SYNAPSE lateral inhibition competitor-set size.
    #[serde(default = "default_inhibition_m")]
    pub inhibition_m: usize,

    /// SYNAPSE sigmoid steepness.
    #[serde(default = "default_sigmoid_gamma")]
    pub sigmoid_gamma: f32,

    /// SYNAPSE confidence gate threshold.
    #[serde(default = "default_gate_tau")]
    pub gate_tau: f32,

    /// Final top-K cutoff for activation results.
    #[serde(default = "default_top_k")]
    pub top_k: usize,

    /// If true, fall back to Ollama 3B intent classifier for ambiguous queries.
    /// Default false: rule-based classifier handles ≥80% of queries; LLM only
    /// for genuinely ambiguous ones. Deferred to P7+; flag exists for early
    /// experimentation.
    #[serde(default)]
    pub llm_intent_fallback: bool,
}

fn default_ppr_damping() -> f32 { 0.5 }
fn default_ppr_tolerance() -> f32 { 1e-4 }
fn default_ppr_max_iter() -> usize { 50 }
fn default_subgraph_cap() -> usize { 10_000 }
fn default_inhibition_beta() -> f32 { 0.15 }
fn default_inhibition_m() -> usize { 7 }
fn default_sigmoid_gamma() -> f32 { 5.0 }
fn default_gate_tau() -> f32 { 0.12 }
fn default_top_k() -> usize { 30 }

impl Default for ActivationConfig {
    fn default() -> Self {
        Self {
            damping: default_ppr_damping(),
            tolerance: default_ppr_tolerance(),
            max_iter: default_ppr_max_iter(),
            subgraph_cap: default_subgraph_cap(),
            inhibition_beta: default_inhibition_beta(),
            inhibition_m: default_inhibition_m(),
            sigmoid_gamma: default_sigmoid_gamma(),
            gate_tau: default_gate_tau(),
            top_k: default_top_k(),
            llm_intent_fallback: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    /// Unique device identifier
    pub id: String,

    /// Human-readable device name
    pub name: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    /// Hub WebSocket URL (e.g., wss://agent-hub.example.com/ws)
    pub hub_url: Option<String>,

    /// JWT authentication token
    pub auth_token: Option<String>,

    /// Heartbeat interval in seconds
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval: u32,

    /// Reconnect settings
    #[serde(default)]
    pub reconnect: ReconnectConfig,
}

impl std::fmt::Debug for ConnectionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionConfig")
            .field("hub_url", &self.hub_url)
            .field("auth_token", &self.auth_token.as_ref().map(|_| "[REDACTED]"))
            .field("heartbeat_interval", &self.heartbeat_interval)
            .field("reconnect", &self.reconnect)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconnectConfig {
    /// Enable automatic reconnection
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Initial delay in seconds
    #[serde(default = "default_initial_delay")]
    pub initial_delay: f32,

    /// Maximum delay in seconds
    #[serde(default = "default_max_delay")]
    pub max_delay: f32,

    /// Maximum attempts (0 = infinite)
    #[serde(default)]
    pub max_attempts: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Allowed paths for file access
    #[serde(default)]
    pub allowed_paths: Vec<String>,

    /// Blocked file patterns (glob)
    #[serde(default = "default_blocked_patterns")]
    pub blocked_patterns: Vec<String>,

    /// Maximum file size to index (bytes)
    #[serde(default = "default_max_file_size")]
    pub max_file_size: u64,

    /// Allow read access
    #[serde(default = "default_true")]
    pub allow_read: bool,

    /// Allow write access (default: false)
    #[serde(default)]
    pub allow_write: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    /// Local web UI port
    #[serde(default = "default_ui_port")]
    pub port: u16,

    /// Enable local web UI
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexingConfig {
    /// Enable file watching for auto-reindex
    #[serde(default = "default_true")]
    pub watch_enabled: bool,

    /// Debounce time for file changes (ms)
    #[serde(default = "default_debounce")]
    pub debounce_ms: u32,

    /// File extensions to index (empty = all text files)
    #[serde(default)]
    pub file_extensions: Vec<String>,

    /// Include content snippets in results
    #[serde(default = "default_true")]
    pub include_content: bool,

    /// Maximum file size to index (bytes)
    #[serde(default = "default_max_file_size")]
    pub max_file_size: u64,

    /// Exclude paths
    #[serde(default)]
    pub exclude_paths: Vec<String>,

    /// Exclude patterns (glob)
    #[serde(default)]
    pub exclude_patterns: Vec<String>,

    /// Follow symlinks
    #[serde(default)]
    pub follow_symlinks: bool,

    /// Max directory depth
    #[serde(default)]
    pub max_depth: Option<usize>,
}

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct K2KConfig {
    /// Enable K2K federation server
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// K2K server port
    #[serde(default = "default_k2k_port")]
    pub port: u16,

    /// Require client registration (recommended)
    #[serde(default = "default_true")]
    pub require_client_registration: bool,

    /// Allowed client IDs (empty = allow all registered clients)
    #[serde(default)]
    pub allowed_clients: Vec<String>,

    /// Per-client write grants for non-personal stores.
    /// Map of client_id → list of store_ids the client may write to.
    /// Personal stores (store_type == "personal", owner_id == client_id) are
    /// always writable by their owner and do not need an explicit entry here.
    #[serde(default)]
    pub client_store_grants: std::collections::HashMap<String, Vec<String>>,

    /// Secret required for auto-approved client registration (PSK).
    /// If None, all registrations go to "pending" (admin must approve).
    /// If set, clients that provide the correct secret are auto-approved.
    #[serde(default)]
    pub registration_secret: Option<String>,

    /// Allowed CORS origins (empty = localhost only)
    #[serde(default)]
    pub allowed_origins: Vec<String>,

    /// Per-client task rate limit: maximum tasks per window per client.
    /// Default: 10 tasks per 60-second window.
    #[serde(default = "default_per_client_task_limit")]
    pub per_client_rate_limit: u32,

    /// Sliding window duration in seconds for per-client rate limiting.
    #[serde(default = "default_per_client_window_seconds")]
    pub per_client_window_seconds: u64,

    /// Per-client storage quota: maximum total articles a client may store
    /// across all their knowledge stores. 0 = unlimited.
    /// Default: 10,000 articles.
    #[serde(default = "default_per_client_article_quota")]
    pub per_client_article_quota: usize,
}

fn default_per_client_task_limit() -> u32 {
    10
}
fn default_per_client_window_seconds() -> u64 {
    60
}
fn default_per_client_article_quota() -> usize {
    10_000
}

impl std::fmt::Debug for K2KConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("K2KConfig")
            .field("enabled", &self.enabled)
            .field("port", &self.port)
            .field("require_client_registration", &self.require_client_registration)
            .field("allowed_clients", &self.allowed_clients)
            .field("client_store_grants", &self.client_store_grants)
            .field("registration_secret", &self.registration_secret.as_ref().map(|_| "[REDACTED]"))
            .field("allowed_origins", &self.allowed_origins)
            .field("per_client_rate_limit", &self.per_client_rate_limit)
            .field("per_client_window_seconds", &self.per_client_window_seconds)
            .field("per_client_article_quota", &self.per_client_article_quota)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MicroBackendConfig {
    /// Enable standalone micro-backend mode
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryConfig {
    /// Enable mDNS discovery
    #[serde(default)]
    pub enabled: bool,

    /// Auto-federate with discovered nodes
    #[serde(default)]
    pub auto_federate: bool,

    /// Browse interval in seconds
    #[serde(default = "default_browse_interval")]
    pub browse_interval_secs: u64,

    /// Allowlist of trusted peer node IDs.
    /// When non-empty, only mDNS-discovered nodes whose `node_id` appears in
    /// this list will be registered as federation peers.  An empty list (the
    /// default) accepts no discovered nodes, preventing unauthenticated peers
    /// from injecting themselves into federated searches.
    #[serde(default)]
    pub trusted_node_ids: Vec<String>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            auto_federate: false,
            browse_interval_secs: default_browse_interval(),
            trusted_node_ids: Vec::new(),
        }
    }
}

fn default_browse_interval() -> u64 {
    30
}

// Default value functions
fn default_heartbeat_interval() -> u32 {
    30
}
fn default_initial_delay() -> f32 {
    1.0
}
fn default_max_delay() -> f32 {
    60.0
}
fn default_ui_port() -> u16 {
    19850
}
fn default_debounce() -> u32 {
    500
}
fn default_true() -> bool {
    true
}

fn default_blocked_patterns() -> Vec<String> {
    vec![
        "*.env".into(),
        "*.env.*".into(),
        "**/.git/**".into(),
        "**/node_modules/**".into(),
        "**/__pycache__/**".into(),
        "**/venv/**".into(),
        "*.key".into(),
        "*.pem".into(),
        "*.p12".into(),
        "**/.ssh/**".into(),
        "**/.aws/**".into(),
        "**/credentials*".into(),
        "**/secrets*".into(),
    ]
}

fn default_max_file_size() -> u64 {
    10 * 1024 * 1024
} // 10 MB
fn default_k2k_port() -> u16 {
    8765
}

impl Default for Config {
    fn default() -> Self {
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        Self {
            device: DeviceConfig {
                id: format!("{}-{}", hostname, &Uuid::new_v4().to_string()[..8]),
                name: hostname,
            },
            connection: ConnectionConfig {
                hub_url: None,
                auth_token: None,
                heartbeat_interval: default_heartbeat_interval(),
                reconnect: ReconnectConfig::default(),
            },
            security: SecurityConfig {
                allowed_paths: default_allowed_paths(),
                blocked_patterns: default_blocked_patterns(),
                max_file_size: default_max_file_size(),
                allow_read: true,
                allow_write: false,
            },
            ui: UiConfig {
                port: default_ui_port(),
                enabled: true,
            },
            indexing: IndexingConfig {
                watch_enabled: true,
                debounce_ms: default_debounce(),
                file_extensions: vec![],
                include_content: true,
                max_file_size: default_max_file_size(),
                exclude_paths: vec![],
                exclude_patterns: vec![],
                follow_symlinks: false,
                max_depth: None,
            },
            k2k: K2KConfig {
                enabled: true,
                port: default_k2k_port(),
                require_client_registration: true,
                allowed_clients: vec![],
                client_store_grants: std::collections::HashMap::new(),
                registration_secret: None,
                allowed_origins: vec![],
                per_client_rate_limit: default_per_client_task_limit(),
                per_client_window_seconds: default_per_client_window_seconds(),
                per_client_article_quota: default_per_client_article_quota(),
            },
            micro_backend: MicroBackendConfig::default(),
            discovery: DiscoveryConfig::default(),
            web_search: WebSearchConfig::default(),
            extraction: ExtractionConfig::default(),
            retrieval: RetrievalConfig::default(),
            graph: GraphConfig::default(),
            reflection: ReflectionConfig::default(),
            decay: DecayConfig::default(),
            compaction: CompactionConfig::default(),
            agent_api: AgentApiConfig::default(),
            policy: PolicyConfig::default(),
        }
    }
}

impl Default for ReconnectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            initial_delay: default_initial_delay(),
            max_delay: default_max_delay(),
            max_attempts: 0,
        }
    }
}

/// Get platform-appropriate default paths
fn default_allowed_paths() -> Vec<String> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));

    #[cfg(target_os = "windows")]
    {
        // Modern Windows often redirects Documents and Desktop to OneDrive
        // via "Known Folder Move". Include both local and OneDrive paths,
        // filtering to only those that actually exist on this machine.
        let candidates = [
            home.join("Documents"),
            home.join("Desktop"),
            home.join("OneDrive").join("Documents"),
            home.join("OneDrive").join("Desktop"),
        ];
        let paths: Vec<String> = candidates
            .iter()
            .filter(|p| p.is_dir())
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        if paths.is_empty() {
            // Fallback if nothing exists yet
            vec![home.join("Documents").to_string_lossy().to_string()]
        } else {
            paths
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        vec![
            home.join("Documents").to_string_lossy().to_string(),
            home.join("Projects").to_string_lossy().to_string(),
        ]
    }
}

/// Get project directories
fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("", "", "knowledge-nexus-agent")
}

/// Get configuration file path
pub fn config_path() -> PathBuf {
    project_dirs()
        .map(|d| d.config_dir().join("config.yaml"))
        .unwrap_or_else(|| PathBuf::from("config.yaml"))
}

/// Get configuration directory path
pub fn config_dir() -> PathBuf {
    project_dirs()
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Get data directory path
pub fn data_dir() -> PathBuf {
    project_dirs()
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Get log file path
pub fn log_path() -> PathBuf {
    data_dir().join("agent.log")
}

/// Get PID file path
pub fn pid_path() -> PathBuf {
    data_dir().join("agent.pid")
}

/// Get vector database path
#[allow(dead_code)]
pub fn vectordb_path() -> PathBuf {
    data_dir().join("vectors")
}

/// Get SQLite database path
pub fn sqlite_path() -> PathBuf {
    config_dir().join("kn.db")
}

/// Get the capabilities mesh config file path
pub fn capabilities_config_path() -> PathBuf {
    config_dir().join("capabilities.yaml")
}

/// Load the capability mesh configuration from disk.
/// Returns an empty config if the file does not exist.
pub async fn load_capability_mesh_config() -> Result<CapabilityMeshConfig> {
    let path = capabilities_config_path();
    if !path.exists() {
        return Ok(CapabilityMeshConfig::default());
    }
    let contents = fs::read_to_string(&path)
        .await
        .with_context(|| format!("Failed to read capability config from {:?}", path))?;
    let cfg: CapabilityMeshConfig = serde_yaml::from_str(&contents)
        .with_context(|| format!("Failed to parse capability config from {:?}", path))?;
    Ok(cfg)
}

/// Write PID file
pub fn write_pid() -> Result<()> {
    let path = pid_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, std::process::id().to_string())?;
    Ok(())
}

/// Read PID file
pub fn read_pid() -> Result<Option<u32>> {
    let path = pid_path();
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)?;
    let pid: u32 = content.trim().parse()?;
    Ok(Some(pid))
}

/// Remove PID file
pub fn remove_pid() -> Result<()> {
    let path = pid_path();
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Check if process with given PID is running
pub fn is_process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        use std::process::Command;
        // Use kill -0 to check if process exists without sending a signal
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    #[cfg(windows)]
    {
        // On Windows, use tasklist to check if PID exists
        use std::process::Command;
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid)])
            .output()
            .map(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
            .unwrap_or(false)
    }
}

/// Stop a running process by PID
pub fn stop_process(pid: u32) -> Result<bool> {
    #[cfg(unix)]
    {
        use std::process::Command;
        // Send SIGTERM first for graceful shutdown
        let result = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()?;

        if result.success() {
            // Wait a bit and check if process stopped
            std::thread::sleep(std::time::Duration::from_millis(500));
            if is_process_running(pid) {
                // Force kill if still running
                Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .status()?;
            }
            remove_pid()?;
            return Ok(true);
        }
        Ok(false)
    }

    #[cfg(windows)]
    {
        use std::process::Command;
        let result = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .status()?;
        if result.success() {
            remove_pid()?;
        }
        Ok(result.success())
    }
}

fn first_env(keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| std::env::var(k).ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn parse_csv_env(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .collect()
}

/// Load configuration from file
pub async fn load_config() -> Result<Config> {
    let path = config_path();

    let mut config = if path.exists() {
        let contents = fs::read_to_string(&path)
            .await
            .with_context(|| format!("Failed to read config from {:?}", path))?;

        serde_yaml::from_str(&contents)
            .with_context(|| format!("Failed to parse config from {:?}", path))?
    } else {
        Config::default()
    };

    // Override with environment variables

    if let Some(url) = first_env(&["KN_HUB_URL", "K2K_HUB_URL"]) {
        config.connection.hub_url = Some(url);
    }

    if let Some(token) = first_env(&["KN_AUTH_TOKEN", "K2K_AUTH_TOKEN"]) {
        config.connection.auth_token = Some(token);
    }

    if let Some(device_id) = first_env(&["KN_DEVICE_ID", "K2K_DEVICE_ID"]) {
        config.device.id = device_id;
    }

    if let Some(device_name) = first_env(&["KN_DEVICE_NAME", "K2K_DEVICE_NAME"]) {
        config.device.name = device_name;
    }

    if let Some(paths) = first_env(&["KN_ALLOWED_PATHS", "K2K_ALLOWED_PATHS"]) {
        let parsed = parse_csv_env(&paths);
        if !parsed.is_empty() {
            config.security.allowed_paths = parsed;
        }
    }

    if let Some(patterns) = first_env(&["KN_BLOCKED_PATTERNS", "K2K_BLOCKED_PATTERNS"]) {
        let parsed = parse_csv_env(&patterns);
        if !parsed.is_empty() {
            config.security.blocked_patterns = parsed;
        }
    }

    if let Some(ui_enabled) = first_env(&["KN_UI_ENABLED", "K2K_UI_ENABLED"]) {
        if let Ok(enabled) = ui_enabled.parse::<bool>() {
            config.ui.enabled = enabled;
        }
    }

    // Web search: resolve API key from env vars if not set in config file
    if config.web_search.api_key.is_none() {
        let env_key = match config.web_search.provider.as_str() {
            "google" => first_env(&["GOOGLE_SEARCH_API_KEY"]),
            "bing" => first_env(&["BING_SEARCH_API_KEY"]),
            _ => first_env(&["BRAVE_SEARCH_API_KEY"]),
        };
        if env_key.is_some() {
            config.web_search.api_key = env_key;
            // If an API key is present, enable web search automatically
            config.web_search.enabled = true;
        }
    }

    // Extraction config overrides
    if let Some(url) = first_env(&["KN_OLLAMA_URL"]) {
        config.extraction.ollama_url = url;
    }
    if let Some(model) = first_env(&["KN_EXTRACTION_MODEL"]) {
        config.extraction.model = model;
    }
    if let Some(enabled) = first_env(&["KN_EXTRACTION_ENABLED"]) {
        if let Ok(v) = enabled.parse::<bool>() {
            config.extraction.enabled = v;
        }
    }

    Ok(config)
}

/// Restrict config file permissions (cross-platform, inline for crate independence).
fn restrict_config_file_permissions(path: &std::path::Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    {
        let username = std::env::var("USERNAME").unwrap_or_default();
        if !username.is_empty() {
            let path_str = path.to_string_lossy();
            match std::process::Command::new("icacls")
                .args([&*path_str, "/inheritance:r"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
            {
                Ok(status) if !status.success() => {
                    tracing::warn!("icacls /inheritance:r failed for {}: exit code {:?}", path_str, status.code());
                }
                Err(e) => {
                    tracing::warn!("Failed to run icacls /inheritance:r for {}: {}", path_str, e);
                }
                _ => {}
            }
            let grant = format!("{}:(R,W)", username);
            match std::process::Command::new("icacls")
                .args([&*path_str, "/grant:r", &grant])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
            {
                Ok(status) if !status.success() => {
                    tracing::warn!("icacls /grant:r failed for {}: exit code {:?}", path_str, status.code());
                }
                Err(e) => {
                    tracing::warn!("Failed to run icacls /grant:r for {}: {}", path_str, e);
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Save configuration to file
pub async fn save_config(config: &Config) -> Result<()> {
    let path = config_path();

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).await?;
    }

    let contents = serde_yaml::to_string(config)?;
    fs::write(&path, contents).await?;

    // Set restrictive permissions (cross-platform)
    restrict_config_file_permissions(&path)?;

    Ok(())
}

/// Initialize configuration (create default if not exists)
pub async fn init_config(hub_url: Option<String>, force: bool) -> Result<()> {
    let path = config_path();

    if path.exists() && !force {
        anyhow::bail!(
            "Configuration already exists at {:?}. Use --force to overwrite.",
            path
        );
    }

    let mut config = Config::default();

    if let Some(url) = hub_url {
        config.connection.hub_url = Some(url);
    }

    save_config(&config).await?;

    println!("Configuration created at: {:?}", path);
    println!();
    println!("Next steps:");
    println!("  1. Edit the config to add your paths: knowledge-nexus-agent config");
    #[cfg(windows)]
    println!("  2. Set your auth token: set KN_AUTH_TOKEN=your-token");
    #[cfg(not(windows))]
    println!("  2. Set your auth token: export KN_AUTH_TOKEN=your-token");
    println!("  3. Start the agent: knowledge-nexus-agent start");

    Ok(())
}
