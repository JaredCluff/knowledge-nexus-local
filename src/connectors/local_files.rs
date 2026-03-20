//! LocalFileConnector - wraps the existing VectorDB file indexer as a K2KConnector

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::Mutex;

use super::traits::{ConnectorCapabilities, K2KConnector};
use crate::embeddings::EmbeddingModel;
use crate::k2k::models::K2KResult;
use crate::vectordb::VectorDB;

#[allow(dead_code)]
pub struct LocalFileConnector {
    vectordb: Arc<VectorDB>,
    embedding_model: Arc<Mutex<EmbeddingModel>>,
    device_id: String,
}

impl LocalFileConnector {
    pub fn new(
        vectordb: Arc<VectorDB>,
        embedding_model: Arc<Mutex<EmbeddingModel>>,
        device_id: String,
    ) -> Self {
        Self {
            vectordb,
            embedding_model,
            device_id,
        }
    }
}

#[async_trait]
impl K2KConnector for LocalFileConnector {
    fn connector_type(&self) -> &str {
        "local_files"
    }

    fn name(&self) -> &str {
        "Local File Indexer"
    }

    async fn search(&self, query: &str, embedding: &[f32], limit: usize) -> Result<Vec<K2KResult>> {
        // Use provided embedding if non-empty, otherwise generate one
        let search_embedding = if embedding.is_empty() {
            let mut model = self.embedding_model.lock().await;
            model.embed_text(query)?
        } else {
            embedding.to_vec()
        };

        let results = self.vectordb.search(&search_embedding, limit).await?;

        let k2k_results = results
            .into_iter()
            .map(|r| K2KResult {
                article_id: r.id.clone(),
                store_id: self.device_id.clone(),
                title: r.title.clone(),
                summary: r
                    .chunk_text
                    .as_deref()
                    .map(|t| {
                        if t.len() > 200 {
                            let end = (0..=200)
                                .rev()
                                .find(|&i| t.is_char_boundary(i))
                                .unwrap_or(0);
                            format!("{}...", &t[..end])
                        } else {
                            t.to_string()
                        }
                    })
                    .unwrap_or_default(),
                content: r.chunk_text.unwrap_or_default(),
                confidence: r.score,
                source_type: r.source_type,
                tags: vec![],
                metadata: serde_json::json!({
                    "path": r.path,
                    "size_bytes": r.size_bytes,
                    "modified_at": r.modified_at,
                    "content_type": r.content_type,
                    "document_type": r.document_type,
                    "chunk_index": r.chunk_index,
                }),
                provenance: None,
            })
            .collect();

        Ok(k2k_results)
    }

    fn capabilities(&self) -> ConnectorCapabilities {
        ConnectorCapabilities {
            can_search: true,
            can_list: true,
            can_read: true,
            can_write: false,
        }
    }

    async fn is_healthy(&self) -> bool {
        self.vectordb.count().await.is_ok()
    }
}
