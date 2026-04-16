//! WebClipReceiver - HTTP endpoint for browser extensions to push web clips as articles

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::info;
use uuid::Uuid;

use super::traits::{ConnectorCapabilities, K2KConnector};
use crate::store::{Article, Store};
use crate::k2k::models::K2KResult;
use crate::knowledge::ArticleService;

/// A web clip submitted by a browser extension
#[derive(Debug, Deserialize)]
pub struct WebClipRequest {
    pub url: String,
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub store_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WebClipResponse {
    pub article_id: String,
    pub message: String,
}

pub struct WebClipReceiver {
    db: Arc<dyn Store>,
    article_service: Arc<ArticleService>,
}

impl WebClipReceiver {
    pub fn new(db: Arc<dyn Store>, article_service: Arc<ArticleService>) -> Self {
        Self {
            db,
            article_service,
        }
    }

    /// Ingest a web clip, creating an article in the user's default store
    pub async fn ingest(&self, clip: &WebClipRequest) -> Result<WebClipResponse> {
        const MAX_CLIP_CONTENT_BYTES: usize = 10 * 1024 * 1024; // 10 MB
        if clip.content.len() > MAX_CLIP_CONTENT_BYTES {
            anyhow::bail!(
                "Web clip content exceeds maximum size of {} bytes",
                MAX_CLIP_CONTENT_BYTES
            );
        }

        // Validate URL scheme to prevent XSS via javascript:, data:, or file: URLs
        // being stored and later rendered as links in a web UI
        let lower_url = clip.url.to_lowercase();
        if !lower_url.starts_with("http://") && !lower_url.starts_with("https://") {
            anyhow::bail!("Web clip URL must use http or https scheme");
        }

        // Find target store: explicit or default personal store
        let store_id = match &clip.store_id {
            Some(id) => id.clone(),
            None => {
                let owner = self
                    .db
                    .get_owner_user()
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("No owner user found"))?;
                let stores = self.db.list_stores_for_user(&owner.id).await?;
                stores
                    .into_iter()
                    .find(|s| s.store_type == "personal")
                    .map(|s| s.id)
                    .ok_or_else(|| anyhow::anyhow!("No personal store found"))?
            }
        };

        let store = self
            .db
            .get_store(&store_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Store not found: {}", store_id))?;

        let now = chrono::Utc::now().to_rfc3339();
        let article_id = Uuid::new_v4().to_string();
        let content_hash = crate::store::hash::content_hash(&clip.content);

        let article = Article {
            id: article_id.clone(),
            store_id,
            title: clip.title.clone(),
            content: clip.content.clone(),
            source_type: "web_clip".to_string(),
            source_id: clip.url.clone(),
            content_hash,
            tags: serde_json::json!(clip.tags),
            embedded_at: None,
            created_at: now.clone(),
            updated_at: now,
        };

        self.article_service
            .create(&article, &store.lancedb_collection)
            .await?;

        info!("Ingested web clip: {} ({})", clip.title, clip.url);

        Ok(WebClipResponse {
            article_id,
            message: format!("Web clip '{}' saved successfully", clip.title),
        })
    }
}

#[async_trait]
impl K2KConnector for WebClipReceiver {
    fn connector_type(&self) -> &str {
        "web_clip"
    }

    fn name(&self) -> &str {
        "Web Clip Receiver"
    }

    async fn search(
        &self,
        _query: &str,
        _embedding: &[f32],
        _limit: usize,
    ) -> Result<Vec<K2KResult>> {
        // Web clips are stored as articles, searched via the article store
        // This connector is primarily a write connector
        Ok(vec![])
    }

    fn capabilities(&self) -> ConnectorCapabilities {
        ConnectorCapabilities {
            can_search: false,
            can_list: false,
            can_read: false,
            can_write: true,
        }
    }

    async fn is_healthy(&self) -> bool {
        true
    }
}
