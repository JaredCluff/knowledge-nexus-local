use std::sync::Arc;

use anyhow::Result;

use crate::store::{Article, Store, Message};
use crate::knowledge::ArticleService;

pub struct KnowledgeExtractor {
    db: Arc<dyn Store>,
    article_service: Arc<ArticleService>,
}

impl KnowledgeExtractor {
    pub fn new(db: Arc<dyn Store>, article_service: Arc<ArticleService>) -> Self {
        Self {
            db,
            article_service,
        }
    }

    /// Extract knowledge from a conversation and save as an article
    pub async fn extract_from_conversation(
        &self,
        conversation_id: &str,
        store_id: &str,
        store_collection: &str,
        title: Option<String>,
    ) -> Result<Article> {
        let messages = self.db.list_messages_for_conversation(conversation_id).await?;
        let conv = self
            .db
            .get_conversation(conversation_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Conversation not found: {}", conversation_id))?;

        // Build article content from messages
        let content = Self::messages_to_content(&messages);
        let article_title = title.unwrap_or_else(|| format!("Knowledge from: {}", conv.title));
        let content_hash = crate::store::hash::content_hash(&content);

        let now = chrono::Utc::now().to_rfc3339();
        let article = Article {
            id: uuid::Uuid::new_v4().to_string(),
            store_id: store_id.to_string(),
            title: article_title,
            content,
            source_type: "conversation_extract".to_string(),
            source_id: format!("conversation://{}", conversation_id),
            content_hash,
            tags: serde_json::json!(["extracted", "conversation"]),
            embedded_at: None,
            created_at: now.clone(),
            updated_at: now,
        };

        match self.article_service.create(&article, store_collection).await? {
            crate::knowledge::articles::CreateResult::Created => {}
            crate::knowledge::articles::CreateResult::Duplicate { existing_id } => {
                tracing::info!(
                    "Conversation extract was a duplicate of article {}",
                    existing_id
                );
            }
        }

        Ok(article)
    }

    fn messages_to_content(messages: &[Message]) -> String {
        let mut content = String::new();

        for msg in messages {
            // Skip system messages
            if msg.role == "system" {
                continue;
            }

            let role_label = match msg.role.as_str() {
                "user" => "User",
                "assistant" => "Assistant",
                _ => &msg.role,
            };

            content.push_str(&format!("**{}**: {}\n\n", role_label, msg.content));
        }

        content
    }
}
