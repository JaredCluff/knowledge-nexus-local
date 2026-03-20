//! Remote capability handler for K2K service mesh
//!
//! Routes task inputs to external HTTP endpoints and returns the response as TaskResult.

use anyhow::{Context, Result};
use async_trait::async_trait;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::tasks::TaskHandler;

/// Authentication type for remote capability endpoints
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteAuthType {
    None,
    BearerToken(String),
    ApiKey { header: String, value: String },
}

impl Default for RemoteAuthType {
    fn default() -> Self {
        Self::None
    }
}

/// A task handler that forwards requests to a remote HTTP endpoint.
pub struct RemoteCapabilityHandler {
    pub endpoint_url: String,
    pub http_method: String,
    pub auth_type: RemoteAuthType,
    pub timeout_secs: u64,
    /// Map task input field names to endpoint parameter names (optional rename).
    pub input_mapping: Option<HashMap<String, String>>,
    /// Map response body field names to task result field names (optional rename).
    pub output_mapping: Option<HashMap<String, String>>,
}

impl RemoteCapabilityHandler {
    pub fn new(
        endpoint_url: String,
        http_method: String,
        auth_type: RemoteAuthType,
        timeout_secs: u64,
        input_mapping: Option<HashMap<String, String>>,
        output_mapping: Option<HashMap<String, String>>,
    ) -> Self {
        Self {
            endpoint_url,
            http_method,
            auth_type,
            timeout_secs,
            input_mapping,
            output_mapping,
        }
    }

    /// Remap fields in a JSON object according to a mapping table.
    /// Keys present in the mapping are renamed; all others are kept as-is.
    fn remap(value: serde_json::Value, mapping: &HashMap<String, String>) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let mut out = serde_json::Map::new();
                for (k, v) in map {
                    let new_key = mapping.get(&k).cloned().unwrap_or(k);
                    out.insert(new_key, v);
                }
                serde_json::Value::Object(out)
            }
            other => other,
        }
    }
}

#[async_trait]
impl TaskHandler for RemoteCapabilityHandler {
    async fn execute(
        &self,
        input: &serde_json::Value,
        progress_tx: mpsc::Sender<u8>,
    ) -> Result<serde_json::Value> {
        let _ = progress_tx.send(5).await;

        // Guard against SSRF: only allow HTTPS endpoints
        if !self.endpoint_url.starts_with("https://") {
            anyhow::bail!(
                "Remote endpoint must use HTTPS (got: {})",
                self.endpoint_url
            );
        }

        // Apply input field remapping if configured
        let body = if let Some(ref mapping) = self.input_mapping {
            Self::remap(input.clone(), mapping)
        } else {
            input.clone()
        };

        debug!(
            "[remote_handler] {} {} body={}",
            self.http_method,
            self.endpoint_url,
            body
        );

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(self.timeout_secs))
            .build()
            .context("Failed to build HTTP client")?;

        let _ = progress_tx.send(20).await;

        let method = self.http_method.to_uppercase();
        let mut req = match method.as_str() {
            "GET" => client.get(&self.endpoint_url),
            "POST" => client.post(&self.endpoint_url),
            "PUT" => client.put(&self.endpoint_url),
            "PATCH" => client.patch(&self.endpoint_url),
            _ => client.post(&self.endpoint_url),
        };

        // Apply auth
        req = match &self.auth_type {
            RemoteAuthType::None => req,
            RemoteAuthType::BearerToken(token) => req.bearer_auth(token),
            RemoteAuthType::ApiKey { header, value } => req.header(header.as_str(), value.as_str()),
        };

        // Attach body for non-GET requests
        if method != "GET" {
            req = req.json(&body);
        }

        let _ = progress_tx.send(40).await;

        let resp = req
            .send()
            .await
            .with_context(|| format!("HTTP call to {} failed", self.endpoint_url))?;

        let status = resp.status();
        let _ = progress_tx.send(80).await;

        if !status.is_success() {
            let err_body = resp.text().await.unwrap_or_default();
            // Sanitize for log injection: strip newlines/carriage returns and truncate
            let safe_body: String = err_body
                .chars()
                .filter(|&c| c != '\n' && c != '\r')
                .take(200)
                .collect();
            warn!(
                "[remote_handler] endpoint {} returned {}: {}",
                self.endpoint_url, status, safe_body
            );
            anyhow::bail!("Remote endpoint returned {}: {}", status, safe_body);
        }

        let response_json: serde_json::Value = resp
            .json()
            .await
            .with_context(|| "Failed to parse JSON response from remote endpoint")?;

        let _ = progress_tx.send(95).await;

        // Apply output field remapping if configured
        let result = if let Some(ref mapping) = self.output_mapping {
            Self::remap(response_json, mapping)
        } else {
            response_json
        };

        Ok(result)
    }
}
