//! Connector Registry - manages all registered connectors

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::RwLock;
use tracing::{debug, info};

use super::traits::K2KConnector;
use crate::k2k::models::K2KResult;

pub struct ConnectorRegistry {
    connectors: RwLock<HashMap<String, Arc<dyn K2KConnector>>>,
}

impl ConnectorRegistry {
    pub fn new() -> Self {
        Self {
            connectors: RwLock::new(HashMap::new()),
        }
    }

    /// Register a connector
    pub async fn register(&self, connector: Arc<dyn K2KConnector>) {
        let name = connector.name().to_string();
        let connector_type = connector.connector_type().to_string();
        self.connectors
            .write()
            .await
            .insert(connector_type.clone(), connector);
        info!("Registered connector: {} (type: {})", name, connector_type);
    }

    /// Unregister a connector by type
    #[allow(dead_code)]
    pub async fn unregister(&self, connector_type: &str) {
        self.connectors.write().await.remove(connector_type);
        info!("Unregistered connector: {}", connector_type);
    }

    /// Search across all registered connectors
    #[allow(dead_code)]
    pub async fn search_all(
        &self,
        query: &str,
        embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<K2KResult>> {
        let connectors = self.connectors.read().await;
        let mut all_results = Vec::new();

        for (ctype, connector) in connectors.iter() {
            if !connector.capabilities().can_search {
                continue;
            }

            if !connector.is_healthy().await {
                debug!("Skipping unhealthy connector: {}", ctype);
                continue;
            }

            match connector.search(query, embedding, limit).await {
                Ok(results) => {
                    debug!("Connector {} returned {} results", ctype, results.len());
                    all_results.extend(results);
                }
                Err(e) => {
                    tracing::warn!("Connector {} search failed: {}", ctype, e);
                }
            }
        }

        Ok(all_results)
    }

    /// List all registered connectors
    pub async fn list(&self) -> Vec<(String, String, bool)> {
        let connectors = self.connectors.read().await;
        let mut list = Vec::new();
        for (ctype, connector) in connectors.iter() {
            let healthy = connector.is_healthy().await;
            list.push((ctype.clone(), connector.name().to_string(), healthy));
        }
        list
    }
}
