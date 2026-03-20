//! Federation Agreement Management

use std::sync::Arc;

use anyhow::Result;
use tracing::info;

use crate::db::{Database, FederationAgreement};

#[allow(dead_code)]
pub struct FederationManager {
    db: Arc<Database>,
}

#[allow(dead_code)]
impl FederationManager {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }

    /// Create a federation agreement with a remote node
    pub fn create_agreement(
        &self,
        local_store_id: &str,
        remote_node_id: &str,
        remote_endpoint: &str,
        access_type: &str,
    ) -> Result<FederationAgreement> {
        let agreement = FederationAgreement {
            id: uuid::Uuid::new_v4().to_string(),
            local_store_id: local_store_id.to_string(),
            remote_node_id: remote_node_id.to_string(),
            remote_endpoint: remote_endpoint.to_string(),
            access_type: access_type.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };

        self.db.create_federation_agreement(&agreement)?;
        info!(
            "Created federation agreement: store {} <-> node {} ({})",
            local_store_id, remote_node_id, access_type
        );

        Ok(agreement)
    }

    /// List all federation agreements
    pub fn list_agreements(&self) -> Result<Vec<FederationAgreement>> {
        self.db.list_federation_agreements()
    }

    /// List agreements for a specific remote node
    pub fn agreements_for_node(&self, remote_node_id: &str) -> Result<Vec<FederationAgreement>> {
        let all = self.db.list_federation_agreements()?;
        Ok(all
            .into_iter()
            .filter(|a| a.remote_node_id == remote_node_id)
            .collect())
    }

    /// Remove a federation agreement
    pub fn remove_agreement(&self, agreement_id: &str) -> Result<()> {
        self.db.delete_federation_agreement(agreement_id)?;
        info!("Removed federation agreement: {}", agreement_id);
        Ok(())
    }

    /// Check if a remote node has read access to a local store
    pub fn has_read_access(&self, remote_node_id: &str, local_store_id: &str) -> Result<bool> {
        let agreements = self.db.list_federation_agreements()?;
        Ok(agreements.iter().any(|a| {
            a.remote_node_id == remote_node_id
                && a.local_store_id == local_store_id
                && (a.access_type == "read" || a.access_type == "readwrite")
        }))
    }
}
