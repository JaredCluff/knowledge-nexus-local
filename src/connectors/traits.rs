//! K2K Connector Trait - mirrors the Python K2KConnector base class

use anyhow::Result;
use async_trait::async_trait;

use crate::k2k::models::K2KResult;

/// Capabilities a connector can declare
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ConnectorCapabilities {
    pub can_search: bool,
    pub can_list: bool,
    pub can_read: bool,
    pub can_write: bool,
}

/// A connector that provides knowledge from an external source
#[async_trait]
pub trait K2KConnector: Send + Sync {
    /// Unique connector type identifier (e.g., "local_files", "web_clip")
    fn connector_type(&self) -> &str;

    /// Human-readable connector name
    fn name(&self) -> &str;

    /// Authenticate with the external service (if needed)
    #[allow(dead_code)]
    async fn authenticate(&mut self) -> Result<()> {
        Ok(()) // Default: no auth needed
    }

    /// Search the connector's data source
    #[allow(dead_code)]
    async fn search(&self, query: &str, embedding: &[f32], limit: usize) -> Result<Vec<K2KResult>>;

    /// Get connector capabilities
    #[allow(dead_code)]
    fn capabilities(&self) -> ConnectorCapabilities;

    /// Check if the connector is healthy
    async fn is_healthy(&self) -> bool;
}
