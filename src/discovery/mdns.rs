//! mDNS Service Discovery for K2K nodes

use std::sync::Arc;

use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tracing::{debug, info, warn};

use super::registry::NodeRegistry;
use crate::config::Config;

const SERVICE_TYPE: &str = "_k2k._tcp.local.";

pub struct MDNSService {
    daemon: ServiceDaemon,
    config: Config,
    registry: Arc<NodeRegistry>,
}

impl MDNSService {
    pub fn new(config: Config, registry: Arc<NodeRegistry>) -> Result<Self> {
        let daemon = ServiceDaemon::new()
            .map_err(|e| anyhow::anyhow!("Failed to create mDNS daemon: {}", e))?;

        Ok(Self {
            daemon,
            config,
            registry,
        })
    }

    /// Advertise this node on the local network
    pub fn advertise(&self) -> Result<()> {
        let raw_name = self.config.device.name.replace(' ', "-").to_lowercase();
        // RFC 6763: mDNS instance names must not exceed 63 bytes
        let instance_name = if raw_name.len() > 63 {
            warn!(
                "Device name '{}' exceeds mDNS 63-byte limit ({}); truncating",
                raw_name,
                raw_name.len()
            );
            // Walk back from byte 63 to find a valid UTF-8 boundary
            let end = (0..=63).rev().find(|&i| raw_name.is_char_boundary(i)).unwrap_or(0);
            raw_name[..end].to_string()
        } else {
            raw_name
        };
        let port = self.config.k2k.port;

        let properties = [
            ("version", env!("CARGO_PKG_VERSION")),
            ("node_id", &self.config.device.id),
            ("scope", "family"),
        ];

        let service_info = ServiceInfo::new(
            SERVICE_TYPE,
            &instance_name,
            &format!("{}._k2k._tcp.local.", instance_name),
            "",
            port,
            &properties[..],
        )
        .map_err(|e| anyhow::anyhow!("Failed to create service info: {}", e))?;

        self.daemon
            .register(service_info)
            .map_err(|e| anyhow::anyhow!("Failed to register mDNS service: {}", e))?;

        info!(
            "Advertising K2K node via mDNS: {}._k2k._tcp.local. on port {}",
            instance_name, port
        );

        Ok(())
    }

    /// Browse for other K2K nodes and update registry
    pub async fn browse_loop(&self) -> Result<()> {
        let receiver = self
            .daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| anyhow::anyhow!("Failed to browse mDNS: {}", e))?;

        let my_node_id = self.config.device.id.clone();
        let browse_interval = self.config.discovery.browse_interval_secs;

        info!(
            "Starting mDNS browse for {} (interval: {}s)",
            SERVICE_TYPE, browse_interval
        );

        loop {
            // Process events with a timeout
            match tokio::time::timeout(
                tokio::time::Duration::from_secs(browse_interval),
                tokio::task::spawn_blocking({
                    let receiver = receiver.clone();
                    move || receiver.recv()
                }),
            )
            .await
            {
                Ok(Ok(Ok(event))) => {
                    self.handle_event(event, &my_node_id).await;
                }
                Ok(Ok(Err(e))) => {
                    warn!("mDNS receive error: {}", e);
                }
                Ok(Err(e)) => {
                    warn!("mDNS task error: {}", e);
                }
                Err(_) => {
                    // Timeout - periodic health check
                    debug!("mDNS browse cycle complete, checking node health");
                    if let Err(e) = self.registry.health_check().await {
                        warn!("Node health check failed: {}", e);
                    }
                }
            }
        }
    }

    async fn handle_event(&self, event: ServiceEvent, my_node_id: &str) {
        match event {
            ServiceEvent::ServiceResolved(info) => {
                let node_id = info
                    .get_properties()
                    .get("node_id")
                    .map(|v| v.val_str().to_string())
                    .unwrap_or_default();

                // Skip our own node
                if node_id == my_node_id {
                    return;
                }

                // Enforce peer allowlist — reject nodes not explicitly trusted.
                let trusted = &self.config.discovery.trusted_node_ids;
                if !trusted.is_empty() && !trusted.contains(&node_id) {
                    warn!(
                        "Ignoring untrusted mDNS peer '{}' — not in discovery.trusted_node_ids",
                        node_id
                    );
                    return;
                }
                if trusted.is_empty() {
                    warn!(
                        "Ignoring mDNS peer '{}' — discovery.trusted_node_ids is empty; \
                         add trusted node IDs to accept federation peers",
                        node_id
                    );
                    return;
                }

                let host = info
                    .get_addresses()
                    .iter()
                    .next()
                    .map(|a| a.to_string())
                    .unwrap_or_else(|| info.get_hostname().to_string());
                let port = info.get_port();

                let version = info
                    .get_properties()
                    .get("version")
                    .map(|v| v.val_str().to_string())
                    .unwrap_or_default();

                info!(
                    "Discovered K2K node: {} at {}:{} (v{})",
                    node_id, host, port, version
                );

                let endpoint = format!("http://{}:{}/k2k/v1", host, port);
                let capabilities = serde_json::json!(["semantic_search", "knowledge_store"]);

                if let Err(e) =
                    self.registry
                        .register_node(&node_id, &host, port, &endpoint, capabilities)
                {
                    warn!("Failed to register discovered node: {}", e);
                }
            }
            ServiceEvent::ServiceRemoved(_type, fullname) => {
                info!("K2K node removed: {}", fullname);
                // We could remove from registry, but the health check will handle stale nodes
            }
            _ => {}
        }
    }

    /// Stop advertising
    #[allow(dead_code)]
    pub fn stop(&self) -> Result<()> {
        self.daemon
            .shutdown()
            .map_err(|e| anyhow::anyhow!("Failed to shutdown mDNS: {}", e))?;
        Ok(())
    }
}
