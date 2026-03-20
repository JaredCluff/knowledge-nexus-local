//! Remote Query Executor - query remote K2K nodes in parallel

use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, info, warn};

use crate::db::DiscoveredNode;
use crate::discovery::NodeRegistry;
use crate::k2k::models::{K2KQueryResponse, K2KResult, ResultProvenance};
use crate::router::executor::StoreSearchResult;

pub struct RemoteQueryExecutor {
    registry: Arc<NodeRegistry>,
    http_client: reqwest::Client,
}

impl RemoteQueryExecutor {
    pub fn new(registry: Arc<NodeRegistry>) -> Result<Self> {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to create HTTP client: {}", e))?;

        Ok(Self {
            registry,
            http_client,
        })
    }

    /// Query all healthy remote nodes in parallel
    pub async fn query_remote_nodes(
        &self,
        query: &str,
        top_k: usize,
    ) -> Result<Vec<StoreSearchResult>> {
        let nodes = self.registry.list_healthy_nodes()?;

        if nodes.is_empty() {
            debug!("No healthy remote nodes to query");
            return Ok(vec![]);
        }

        info!("Querying {} remote nodes", nodes.len());

        let mut handles = Vec::new();
        for node in nodes {
            let client = self.http_client.clone();
            let query = query.to_string();

            handles.push(tokio::spawn(async move {
                Self::query_single_node(&client, &node, &query, top_k).await
            }));
        }

        let mut all_results = Vec::new();
        for handle in handles {
            match handle.await {
                Ok(Ok(result)) => all_results.push(result),
                Ok(Err(e)) => warn!("Remote query failed: {}", e),
                Err(e) => warn!("Remote query task panicked: {}", e),
            }
        }

        Ok(all_results)
    }

    async fn query_single_node(
        client: &reqwest::Client,
        node: &DiscoveredNode,
        query: &str,
        top_k: usize,
    ) -> Result<StoreSearchResult> {
        // Guard against SSRF via malicious mDNS announcements: require HTTPS endpoints
        if !node.endpoint.starts_with("https://") {
            anyhow::bail!(
                "Remote node {} has non-HTTPS endpoint — skipping",
                node.node_id
            );
        }

        let url = format!("{}/route", node.endpoint);

        let body = serde_json::json!({
            "query": query,
            "top_k": top_k,
        });

        let response = client.post(&url).json(&body).send().await?;

        if !response.status().is_success() {
            anyhow::bail!(
                "Remote node {} returned status {}",
                node.node_id,
                response.status()
            );
        }

        let remote_response: K2KQueryResponse = response.json().await?;

        debug!(
            "Got {} results from remote node {}",
            remote_response.results.len(),
            node.node_id
        );

        // Tag results with remote provenance
        let results: Vec<K2KResult> = remote_response
            .results
            .into_iter()
            .enumerate()
            .map(|(rank, mut r)| {
                r.provenance = Some(ResultProvenance {
                    store_id: format!("remote:{}", node.node_id),
                    store_type: "remote".to_string(),
                    original_rank: rank,
                    rrf_score: 0.0,
                });
                r
            })
            .collect();

        Ok(StoreSearchResult {
            store_id: format!("remote:{}", node.node_id),
            store_type: "remote".to_string(),
            results,
        })
    }
}
