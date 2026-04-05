//! Dynamic Capability Registry for K2K multi-agent coordination
//!
//! Capabilities can be registered from:
//! 1. Built-in defaults (semantic_search, file_access, knowledge_store, routing, web_search)
//! 2. A YAML config file (capabilities.yaml) loaded at startup and on hot-reload
//! 3. Auto-discovery from knowledge-nexus microservice manifests
//!
//! Remote-handler capabilities are forwarded via HTTP to the configured endpoint.

use k2k::{AgentCapability, CapabilitiesResponse, CapabilityCategory};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};
use tracing::{info, warn};

use crate::config::{CapabilityEntry, CapabilityHandlerType, RemoteAuthConfig};

/// Richer per-capability metadata surfaced by the manifest endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityInfo {
    /// Core capability fields (from k2k-common)
    #[serde(flatten)]
    pub capability: AgentCapability,
    /// Whether this is a local or remote handler
    pub handler_type: String,
    /// Output JSON schema if defined
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<serde_json::Value>,
    /// Category string (broader than the enum for display)
    pub category_label: String,
    /// Whether the capability is currently reachable (None = not health-checked)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
    /// Health check URL if configured
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health_check_url: Option<String>,
    /// Rate limit in requests per minute (0 = unlimited)
    pub rate_limit_rpm: u32,
    /// Whether streaming results are supported
    pub supports_streaming: bool,
    /// Source: "builtin" | "config" | "discovered"
    pub source: String,
}

/// Internal registration record
#[derive(Clone)]
struct RegistryEntry {
    info: CapabilityInfo,
}

/// Manifest returned from a knowledge-nexus microservice's /.well-known/k2k-manifest
#[derive(Debug, Deserialize)]
pub struct ServiceManifest {
    pub service_name: String,
    pub capabilities: Vec<ManifestCapability>,
}

#[derive(Debug, Deserialize)]
pub struct ManifestCapability {
    pub id: String,
    pub name: String,
    pub category: Option<String>,
    pub description: String,
    pub input_schema: Option<serde_json::Value>,
    pub output_schema: Option<serde_json::Value>,
    pub endpoint: String,
    #[serde(default = "default_method")]
    pub method: String,
    #[serde(default)]
    pub supports_streaming: bool,
}

fn default_method() -> String {
    "POST".to_string()
}

/// Discovered service cache entry
struct DiscoveredService {
    base_url: String,
    _service_name: String,
    discovered_at: Instant,
    #[allow(dead_code)]
    capability_ids: Vec<String>,
}

pub struct CapabilityRegistry {
    node_id: String,
    entries: RwLock<HashMap<String, RegistryEntry>>,
    discovered: RwLock<Vec<DiscoveredService>>,
    discovery_ttl: Duration,
}

impl CapabilityRegistry {
    /// Create a new registry seeded with default built-in capabilities
    pub fn new(node_id: String) -> Self {
        let mut entries = HashMap::new();

        let defaults = vec![
            CapabilityInfo {
                capability: AgentCapability {
                    id: "semantic_search".to_string(),
                    name: "Semantic Search".to_string(),
                    category: CapabilityCategory::Knowledge,
                    description: "Search indexed files using semantic similarity".to_string(),
                    input_schema: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "query": { "type": "string", "description": "Search query" },
                            "top_k": { "type": "integer", "default": 10 }
                        },
                        "required": ["query"]
                    })),
                    version: "1.0.0".to_string(),
                    min_protocol_version: None,
                },
                handler_type: "local".to_string(),
                output_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "results": { "type": "array" },
                        "total": { "type": "integer" }
                    }
                })),
                category_label: "Knowledge".to_string(),
                available: Some(true),
                health_check_url: None,
                rate_limit_rpm: 0,
                supports_streaming: false,
                source: "builtin".to_string(),
            },
            CapabilityInfo {
                capability: AgentCapability {
                    id: "file_access".to_string(),
                    name: "File Access".to_string(),
                    category: CapabilityCategory::Tool,
                    description: "Access and read indexed files from allowed paths".to_string(),
                    input_schema: None,
                    version: "1.0.0".to_string(),
                    min_protocol_version: None,
                },
                handler_type: "local".to_string(),
                output_schema: None,
                category_label: "Tool".to_string(),
                available: Some(true),
                health_check_url: None,
                rate_limit_rpm: 0,
                supports_streaming: false,
                source: "builtin".to_string(),
            },
            CapabilityInfo {
                capability: AgentCapability {
                    id: "knowledge_store".to_string(),
                    name: "Knowledge Store".to_string(),
                    category: CapabilityCategory::Knowledge,
                    description: "Store and retrieve knowledge articles".to_string(),
                    input_schema: None,
                    version: "1.0.0".to_string(),
                    min_protocol_version: None,
                },
                handler_type: "local".to_string(),
                output_schema: None,
                category_label: "Knowledge".to_string(),
                available: Some(true),
                health_check_url: None,
                rate_limit_rpm: 0,
                supports_streaming: false,
                source: "builtin".to_string(),
            },
            CapabilityInfo {
                capability: AgentCapability {
                    id: "routing".to_string(),
                    name: "Query Routing".to_string(),
                    category: CapabilityCategory::Knowledge,
                    description: "Route queries to appropriate knowledge stores".to_string(),
                    input_schema: None,
                    version: "1.0.0".to_string(),
                    min_protocol_version: None,
                },
                handler_type: "local".to_string(),
                output_schema: None,
                category_label: "Knowledge".to_string(),
                available: Some(true),
                health_check_url: None,
                rate_limit_rpm: 0,
                supports_streaming: false,
                source: "builtin".to_string(),
            },
            CapabilityInfo {
                capability: AgentCapability {
                    id: "web_search".to_string(),
                    name: "Web Search".to_string(),
                    category: CapabilityCategory::Tool,
                    description: "Search the web using configured search providers (Brave, Google, Bing)".to_string(),
                    input_schema: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "query": { "type": "string", "description": "Search query" },
                            "max_results": {
                                "type": "integer",
                                "default": 10,
                                "description": "Maximum number of results to return"
                            }
                        },
                        "required": ["query"]
                    })),
                    version: "1.0.0".to_string(),
                    min_protocol_version: None,
                },
                handler_type: "local".to_string(),
                output_schema: None,
                category_label: "Tool".to_string(),
                available: Some(true),
                health_check_url: None,
                rate_limit_rpm: 0,
                supports_streaming: false,
                source: "builtin".to_string(),
            },
        ];

        for info in defaults {
            entries.insert(info.capability.id.clone(), RegistryEntry { info });
        }

        Self {
            node_id,
            entries: RwLock::new(entries),
            discovered: RwLock::new(vec![]),
            discovery_ttl: Duration::from_secs(15 * 60),
        }
    }

    /// Set the discovery TTL from config
    #[allow(dead_code)]
    pub fn set_discovery_ttl_minutes(&mut self, minutes: u64) {
        self.discovery_ttl = Duration::from_secs(minutes * 60);
    }

    /// Load capabilities from config entries.
    /// Returns remote handler pairs for registration with the TaskQueue.
    pub fn load_from_config_entries(
        &self,
        config_entries: Vec<CapabilityEntry>,
    ) -> Vec<(String, crate::k2k::remote_handler::RemoteCapabilityHandler)> {
        let mut remote_handlers = vec![];

        for entry in config_entries {
            let category = parse_category(&entry.category);
            let category_label = entry.category.clone();
            let handler_type_str = match entry.handler_type {
                CapabilityHandlerType::Local => "local",
                CapabilityHandlerType::Remote => "remote",
            };

            let info = CapabilityInfo {
                capability: AgentCapability {
                    id: entry.id.clone(),
                    name: entry.name.clone(),
                    category,
                    description: entry.description.clone(),
                    input_schema: entry.input_schema.clone(),
                    version: "1.0.0".to_string(),
                    min_protocol_version: None,
                },
                handler_type: handler_type_str.to_string(),
                output_schema: entry.output_schema.clone(),
                category_label,
                available: None,
                health_check_url: entry.health_check_url.clone(),
                rate_limit_rpm: entry.rate_limit_rpm,
                supports_streaming: entry.supports_streaming,
                source: "config".to_string(),
            };

            {
                let mut map = self.entries.write().unwrap_or_else(|e| e.into_inner());
                map.insert(entry.id.clone(), RegistryEntry { info });
            }

            // Build remote handler
            if matches!(entry.handler_type, CapabilityHandlerType::Remote) {
                if let Some(endpoint_url) = entry.endpoint_url {
                    let auth = convert_auth(entry.auth);
                    let handler = crate::k2k::remote_handler::RemoteCapabilityHandler::new(
                        endpoint_url,
                        entry.http_method,
                        auth,
                        entry.timeout_secs,
                        entry.input_mapping,
                        entry.output_mapping,
                    );
                    remote_handlers.push((entry.id, handler));
                } else {
                    warn!(
                        "[capabilities] Remote capability '{}' has no endpoint_url — skipping handler",
                        entry.id
                    );
                }
            }
        }

        remote_handlers
    }

    /// Register a new capability (legacy: just AgentCapability, no extra metadata)
    #[allow(dead_code)]
    pub fn register(&self, capability: AgentCapability) -> Result<(), String> {
        let info = CapabilityInfo {
            capability: capability.clone(),
            handler_type: "local".to_string(),
            output_schema: None,
            category_label: format!("{:?}", capability.category),
            available: Some(true),
            health_check_url: None,
            rate_limit_rpm: 0,
            supports_streaming: false,
            source: "builtin".to_string(),
        };
        let mut map = self
            .entries
            .write()
            .map_err(|e| format!("Lock poisoned: {}", e))?;
        map.insert(capability.id.clone(), RegistryEntry { info });
        Ok(())
    }

    /// Register a new capability with full metadata
    #[allow(dead_code)]
    pub fn register_info(&self, info: CapabilityInfo) -> Result<(), String> {
        let mut map = self
            .entries
            .write()
            .map_err(|e| format!("Lock poisoned: {}", e))?;
        map.insert(info.capability.id.clone(), RegistryEntry { info });
        Ok(())
    }

    /// Unregister a capability by ID
    #[allow(dead_code)]
    pub fn unregister(&self, id: &str) -> Result<bool, String> {
        let mut map = self
            .entries
            .write()
            .map_err(|e| format!("Lock poisoned: {}", e))?;
        Ok(map.remove(id).is_some())
    }

    /// List all registered capabilities (basic AgentCapability)
    pub fn list(&self) -> Vec<AgentCapability> {
        let map = self.entries.read().unwrap_or_else(|e| e.into_inner());
        map.values().map(|e| e.info.capability.clone()).collect()
    }

    /// List all registered capabilities with full metadata
    pub fn list_full(&self) -> Vec<CapabilityInfo> {
        let map = self.entries.read().unwrap_or_else(|e| e.into_inner());
        map.values().map(|e| e.info.clone()).collect()
    }

    /// Get the full capabilities response (for the standard endpoint)
    pub fn get_response(&self) -> CapabilitiesResponse {
        CapabilitiesResponse {
            node_id: self.node_id.clone(),
            capabilities: self.list(),
            protocol_version: "1.0".to_string(),
        }
    }

    /// Get capability IDs as strings (for backward-compat health endpoint)
    pub fn capability_ids(&self) -> Vec<String> {
        let map = self.entries.read().unwrap_or_else(|e| e.into_inner());
        map.keys().cloned().collect()
    }

    /// Check if a capability exists
    #[allow(dead_code)]
    pub fn has_capability(&self, id: &str) -> bool {
        let map = self.entries.read().unwrap_or_else(|e| e.into_inner());
        map.contains_key(id)
    }

    /// Register capabilities discovered from a remote service manifest.
    /// Returns remote handlers to register with the TaskQueue.
    pub fn register_discovered(
        &self,
        base_url: &str,
        manifest: ServiceManifest,
    ) -> Vec<(String, crate::k2k::remote_handler::RemoteCapabilityHandler)> {
        let mut remote_handlers = vec![];
        let mut cap_ids = vec![];

        for mc in manifest.capabilities {
            let category = parse_category(mc.category.as_deref().unwrap_or("Knowledge"));
            let category_label = mc.category.unwrap_or_else(|| "Knowledge".to_string());

            let endpoint_url = if mc.endpoint.starts_with("http") {
                mc.endpoint.clone()
            } else {
                format!("{}{}", base_url.trim_end_matches('/'), mc.endpoint)
            };

            // Reject non-HTTPS endpoints at registration time (defense-in-depth;
            // RemoteCapabilityHandler also checks at execution time).
            if !endpoint_url.starts_with("https://") {
                tracing::warn!(
                    capability_id = %mc.id,
                    endpoint = %endpoint_url,
                    "Skipping capability with non-HTTPS endpoint"
                );
                continue;
            }

            let info = CapabilityInfo {
                capability: AgentCapability {
                    id: mc.id.clone(),
                    name: mc.name.clone(),
                    category,
                    description: mc.description.clone(),
                    input_schema: mc.input_schema.clone(),
                    version: "1.0.0".to_string(),
                    min_protocol_version: None,
                },
                handler_type: "remote".to_string(),
                output_schema: mc.output_schema,
                category_label,
                available: Some(true),
                health_check_url: None,
                rate_limit_rpm: 0,
                supports_streaming: mc.supports_streaming,
                source: "discovered".to_string(),
            };

            let handler = crate::k2k::remote_handler::RemoteCapabilityHandler::new(
                endpoint_url,
                mc.method,
                crate::k2k::remote_handler::RemoteAuthType::None,
                30,
                None,
                None,
            );

            cap_ids.push(mc.id.clone());
            remote_handlers.push((mc.id.clone(), handler));

            {
                let mut map = self.entries.write().unwrap_or_else(|e| e.into_inner());
                map.insert(mc.id, RegistryEntry { info });
            }
        }

        {
            let mut disc = self.discovered.write().unwrap_or_else(|e| e.into_inner());
            disc.retain(|d| d.base_url != base_url);
            disc.push(DiscoveredService {
                base_url: base_url.to_string(),
                _service_name: manifest.service_name,
                discovered_at: Instant::now(),
                capability_ids: cap_ids,
            });
        }

        remote_handlers
    }

    /// Remove stale discovered capabilities that are past TTL
    #[allow(dead_code)]
    pub fn evict_stale_discovered(&self) {
        let ttl = self.discovery_ttl;
        let mut disc = self.discovered.write().unwrap_or_else(|e| e.into_inner());
        let mut ids_to_remove = vec![];

        disc.retain(|d| {
            if d.discovered_at.elapsed() > ttl {
                ids_to_remove.extend(d.capability_ids.clone());
                false
            } else {
                true
            }
        });

        if !ids_to_remove.is_empty() {
            let mut map = self.entries.write().unwrap_or_else(|e| e.into_inner());
            for id in &ids_to_remove {
                map.remove(id);
            }
            info!(
                "[capabilities] Evicted {} stale discovered capabilities",
                ids_to_remove.len()
            );
        }
    }

    /// Whether the given service base_url needs re-discovery (past TTL or never discovered)
    pub fn needs_rediscovery(&self, base_url: &str) -> bool {
        let disc = self.discovered.read().unwrap_or_else(|e| e.into_inner());
        disc.iter()
            .find(|d| d.base_url == base_url)
            .map(|d| d.discovered_at.elapsed() > self.discovery_ttl)
            .unwrap_or(true)
    }

    /// Run health checks for any capability that has a health_check_url.
    #[allow(dead_code)]
    pub async fn run_health_checks(&self) {
        let infos: Vec<(String, String)> = {
            let map = self.entries.read().unwrap_or_else(|e| e.into_inner());
            map.values()
                .filter_map(|e| {
                    e.info
                        .health_check_url
                        .as_ref()
                        .map(|url| (e.info.capability.id.clone(), url.clone()))
                })
                .collect()
        };

        let client = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
        {
            Ok(c) => c,
            Err(_) => return,
        };

        for (id, url) in infos {
            let ok = client
                .get(&url)
                .send()
                .await
                .map(|r| r.status().is_success())
                .unwrap_or(false);
            let mut map = self.entries.write().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = map.get_mut(&id) {
                entry.info.available = Some(ok);
            }
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn parse_category(s: &str) -> CapabilityCategory {
    match s.to_lowercase().as_str() {
        "knowledge" | "research" | "storage" => CapabilityCategory::Knowledge,
        "tool" | "communication" => CapabilityCategory::Tool,
        "skill" => CapabilityCategory::Skill,
        "compute" => CapabilityCategory::Compute,
        _ => CapabilityCategory::Knowledge,
    }
}

fn convert_auth(cfg: RemoteAuthConfig) -> crate::k2k::remote_handler::RemoteAuthType {
    use crate::k2k::remote_handler::RemoteAuthType;
    match cfg {
        RemoteAuthConfig::None => RemoteAuthType::None,
        RemoteAuthConfig::BearerToken { token } => RemoteAuthType::BearerToken(token),
        RemoteAuthConfig::ApiKey { header, value } => RemoteAuthType::ApiKey { header, value },
    }
}
