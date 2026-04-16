//! Node Registry - SQLite-backed discovered node tracking with health checks

use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, info, warn};

use crate::store::{Store, DiscoveredNode};

pub struct NodeRegistry {
    db: Arc<dyn Store>,
}

impl NodeRegistry {
    pub fn new(db: Arc<dyn Store>) -> Self {
        Self { db }
    }

    /// Register or update a discovered node
    pub async fn register_node(
        &self,
        node_id: &str,
        host: &str,
        port: u16,
        endpoint: &str,
        capabilities: serde_json::Value,
    ) -> Result<()> {
        let node = DiscoveredNode {
            node_id: node_id.to_string(),
            host: host.to_string(),
            port,
            endpoint: endpoint.to_string(),
            capabilities,
            last_seen: chrono::Utc::now().to_rfc3339(),
            healthy: true,
        };

        self.db.upsert_discovered_node(&node).await?;
        debug!("Registered/updated node: {} at {}", node_id, endpoint);
        Ok(())
    }

    /// List all healthy discovered nodes
    pub async fn list_healthy_nodes(&self) -> Result<Vec<DiscoveredNode>> {
        let nodes = self.db.list_discovered_nodes().await?;
        Ok(nodes.into_iter().filter(|n| n.healthy).collect())
    }

    /// List all discovered nodes
    #[allow(dead_code)]
    pub async fn list_all_nodes(&self) -> Result<Vec<DiscoveredNode>> {
        self.db.list_discovered_nodes().await
    }

    /// Run health checks on all discovered nodes
    pub async fn health_check(&self) -> Result<()> {
        let nodes = self.db.list_discovered_nodes().await?;

        for node in &nodes {
            let healthy = Self::check_node_health(&node.endpoint).await;
            if !healthy && node.healthy {
                warn!("Node {} is now unhealthy", node.node_id);
                self.db.mark_node_unhealthy(&node.node_id).await?;
            } else if healthy && !node.healthy {
                info!("Node {} is now healthy again", node.node_id);
                // Re-register to mark as healthy and update last_seen
                let mut updated = node.clone();
                updated.healthy = true;
                updated.last_seen = chrono::Utc::now().to_rfc3339();
                self.db.upsert_discovered_node(&updated).await?;
            }
        }

        Ok(())
    }

    /// Check if a remote node is healthy by hitting its health endpoint
    async fn check_node_health(endpoint: &str) -> bool {
        let health_url = format!("{}/health", endpoint);
        match reqwest::Client::new()
            .get(&health_url)
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await
        {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    /// Remove a node from the registry
    #[allow(dead_code)]
    pub async fn remove_node(&self, node_id: &str) -> Result<()> {
        self.db.delete_discovered_node(node_id).await?;
        info!("Removed node from registry: {}", node_id);
        Ok(())
    }
}
