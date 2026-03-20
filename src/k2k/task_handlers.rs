//! Built-in task handlers for K2K task delegation

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::info;

use crate::config::WebSearchConfig;
use crate::embeddings::EmbeddingModel;
use crate::vectordb::VectorDB;

use super::tasks::TaskHandler;

/// Handles `semantic_search` tasks by embedding the query and searching the vector database.
pub struct SemanticSearchHandler {
    vectordb: Arc<VectorDB>,
    embedding_model: Arc<Mutex<EmbeddingModel>>,
}

impl SemanticSearchHandler {
    pub fn new(vectordb: Arc<VectorDB>, embedding_model: Arc<Mutex<EmbeddingModel>>) -> Self {
        Self {
            vectordb,
            embedding_model,
        }
    }
}

#[async_trait]
impl TaskHandler for SemanticSearchHandler {
    async fn execute(
        &self,
        input: &serde_json::Value,
        progress_tx: mpsc::Sender<u8>,
    ) -> Result<serde_json::Value> {
        let query = input["query"]
            .as_str()
            .context("Missing 'query' field in task input")?
            .trim();
        if query.is_empty() {
            anyhow::bail!("Query cannot be empty");
        }
        if query.len() > 10_000 {
            anyhow::bail!("Query exceeds maximum length of 10,000 characters");
        }
        let top_k = input["top_k"].as_u64().unwrap_or(5).min(100) as usize;

        info!(
            "[TASK:semantic_search] Query: \"{}\" (top_k: {})",
            query, top_k
        );

        let _ = progress_tx.send(10).await; // Embedding started

        let embedding = {
            let mut model = self.embedding_model.lock().await;
            model
                .embed_text(query)
                .context("Failed to generate embedding")?
        };

        let _ = progress_tx.send(50).await; // Search started

        let results = self
            .vectordb
            .search(&embedding, top_k)
            .await
            .context("Vector search failed")?;

        let _ = progress_tx.send(100).await; // Complete

        Ok(json!({
            "results": results.iter().map(|r| json!({
                "title": r.title,
                "path": r.path,
                "score": r.score,
                "chunk_text": r.chunk_text,
                "source_type": r.source_type,
            })).collect::<Vec<_>>(),
            "total": results.len(),
        }))
    }
}

/// Handles `web_search` tasks by querying a configured search provider API.
pub struct WebSearchHandler {
    config: WebSearchConfig,
    http_client: reqwest::Client,
}

impl WebSearchHandler {
    pub fn new(config: WebSearchConfig) -> Self {
        Self {
            config,
            http_client: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl TaskHandler for WebSearchHandler {
    async fn execute(
        &self,
        input: &serde_json::Value,
        progress_tx: mpsc::Sender<u8>,
    ) -> Result<serde_json::Value> {
        let query = input["query"]
            .as_str()
            .context("Missing 'query' field in task input")?
            .trim();
        if query.is_empty() {
            anyhow::bail!("Query cannot be empty");
        }
        if query.len() > 10_000 {
            anyhow::bail!("Query exceeds maximum length of 10,000 characters");
        }

        let max_results = input["max_results"]
            .as_u64()
            .unwrap_or(self.config.max_results as u64)
            .min(1000) as usize;

        if !self.config.enabled {
            anyhow::bail!(
                "Web search capability is not enabled in System Agent configuration"
            );
        }

        let api_key = self
            .config
            .api_key
            .as_deref()
            .context("Web search is not configured: no API key set. Set BRAVE_SEARCH_API_KEY, GOOGLE_SEARCH_API_KEY, or BING_SEARCH_API_KEY environment variable, or configure web_search.api_key in the agent config file.")?;

        info!(
            "[TASK:web_search] Query: \"{}\" (max_results: {}, provider: {})",
            query, max_results, self.config.provider
        );

        let _ = progress_tx.send(10).await;

        let results = match self.config.provider.as_str() {
            "google" => self.search_google(query, api_key, max_results).await?,
            "bing" => self.search_bing(query, api_key, max_results).await?,
            _ => self.search_brave(query, api_key, max_results).await?,
        };

        let result_count = results.len();
        let _ = progress_tx.send(100).await;

        Ok(json!({
            "results": results,
            "total": result_count,
            "query": query,
            "provider": self.config.provider,
        }))
    }
}

impl WebSearchHandler {
    async fn search_brave(
        &self,
        query: &str,
        api_key: &str,
        max_results: usize,
    ) -> Result<Vec<serde_json::Value>> {
        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
            urlencoding::encode(query),
            max_results.min(20),
        );

        let resp = self
            .http_client
            .get(&url)
            .header("Accept", "application/json")
            .header("Accept-Encoding", "gzip")
            .header("X-Subscription-Token", api_key)
            .send()
            .await
            .context("Brave Search API request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Brave Search API error {}: {}", status, body);
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .context("Failed to parse Brave Search response")?;

        let results = data["web"]["results"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|item| {
                        json!({
                            "title": item["title"].as_str().unwrap_or(""),
                            "url": item["url"].as_str().unwrap_or(""),
                            "content": item["description"].as_str().unwrap_or(""),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok(results)
    }

    async fn search_google(
        &self,
        query: &str,
        api_key: &str,
        max_results: usize,
    ) -> Result<Vec<serde_json::Value>> {
        // Google Custom Search API requires a search engine ID (CX).
        // Store it encoded in the api_key as "key:cx_id" or fall back to env var.
        let (key, cx) = if let Some((k, c)) = api_key.split_once(':') {
            (k.to_string(), c.to_string())
        } else {
            let cx = std::env::var("GOOGLE_SEARCH_CX").unwrap_or_default();
            (api_key.to_string(), cx)
        };

        if cx.is_empty() {
            anyhow::bail!(
                "Google Custom Search requires a Search Engine ID. \
                 Set GOOGLE_SEARCH_CX or encode it in GOOGLE_SEARCH_API_KEY as 'key:cx_id'."
            );
        }

        let url = format!(
            "https://www.googleapis.com/customsearch/v1?key={}&cx={}&q={}&num={}",
            key,
            cx,
            urlencoding::encode(query),
            max_results.min(10),
        );

        let resp = self
            .http_client
            .get(&url)
            .send()
            .await
            .context("Google Custom Search API request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Google Search API error {}: {}", status, body);
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .context("Failed to parse Google Search response")?;

        let results = data["items"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|item| {
                        json!({
                            "title": item["title"].as_str().unwrap_or(""),
                            "url": item["link"].as_str().unwrap_or(""),
                            "content": item["snippet"].as_str().unwrap_or(""),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok(results)
    }

    async fn search_bing(
        &self,
        query: &str,
        api_key: &str,
        max_results: usize,
    ) -> Result<Vec<serde_json::Value>> {
        let url = format!(
            "https://api.bing.microsoft.com/v7.0/search?q={}&count={}",
            urlencoding::encode(query),
            max_results.min(50),
        );

        let resp = self
            .http_client
            .get(&url)
            .header("Ocp-Apim-Subscription-Key", api_key)
            .send()
            .await
            .context("Bing Search API request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Bing Search API error {}: {}", status, body);
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .context("Failed to parse Bing Search response")?;

        let results = data["webPages"]["value"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|item| {
                        json!({
                            "title": item["name"].as_str().unwrap_or(""),
                            "url": item["url"].as_str().unwrap_or(""),
                            "content": item["snippet"].as_str().unwrap_or(""),
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time check that TaskHandler is object-safe
    /// (required by Arc<dyn TaskHandler> in TaskQueue).
    #[test]
    fn test_task_handler_trait_is_object_safe() {
        fn _assert_object_safe(_: &dyn TaskHandler) {}
    }

    /// Full integration test — requires ONNX runtime + LanceDB. Run manually.
    #[test]
    #[ignore]
    fn test_semantic_search_missing_query() {
        // Would need real EmbeddingModel + VectorDB to test.
        // Verifying that missing "query" field returns an error.
    }
}
