use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::Mutex;
use tracing::info;

use crate::store::{Article, Store};
use crate::embeddings::EmbeddingModel;
use crate::vectordb::{ChunkMetadata, DocumentMetadata, VectorDB};

pub struct ArticleService {
    db: Arc<dyn Store>,
    vectordb: Arc<VectorDB>,
    embedding_model: Arc<Mutex<EmbeddingModel>>,
}

impl ArticleService {
    pub fn new(
        db: Arc<dyn Store>,
        vectordb: Arc<VectorDB>,
        embedding_model: Arc<Mutex<EmbeddingModel>>,
    ) -> Self {
        Self {
            db,
            vectordb,
            embedding_model,
        }
    }

    /// Create an article, auto-embed it into vector store
    pub async fn create(&self, article: &Article, store_collection: &str) -> Result<()> {
        self.db.create_article(article).await?;

        // Chunk and embed
        self.embed_article(article, store_collection).await?;

        // Mark as embedded
        let mut updated = article.clone();
        updated.embedded_at = Some(chrono::Utc::now().to_rfc3339());
        updated.updated_at = chrono::Utc::now().to_rfc3339();
        self.db.update_article(&updated).await?;

        info!("Created and embedded article: {}", article.title);
        Ok(())
    }

    /// Update article, re-embed
    pub async fn update(&self, article: &Article, store_collection: &str) -> Result<()> {
        // Delete old vectors
        self.vectordb.delete_document(&article.id).await.ok();

        self.db.update_article(article).await?;

        // Re-embed
        self.embed_article(article, store_collection).await?;

        let mut updated = article.clone();
        updated.embedded_at = Some(chrono::Utc::now().to_rfc3339());
        self.db.update_article(&updated).await?;

        info!("Updated and re-embedded article: {}", article.title);
        Ok(())
    }

    /// Delete article and its vectors
    pub async fn delete(&self, article_id: &str) -> Result<()> {
        self.vectordb.delete_document(article_id).await.ok();
        self.db.delete_article(article_id).await?;
        info!("Deleted article: {}", article_id);
        Ok(())
    }

    pub async fn get(&self, id: &str) -> Result<Option<Article>> {
        self.db.get_article(id).await
    }

    pub async fn list_for_store(&self, store_id: &str) -> Result<Vec<Article>> {
        self.db.list_articles_for_store(store_id).await
    }

    /// Chunk text and embed into vector store
    async fn embed_article(&self, article: &Article, _store_collection: &str) -> Result<()> {
        let chunks = chunk_text(&article.content, 500, 50);
        let now = chrono::Utc::now().to_rfc3339();

        let mut model = self.embedding_model.lock().await;

        let mut chunk_data = Vec::new();
        for (i, chunk_text) in chunks.iter().enumerate() {
            let embedding = model
                .embed_text(chunk_text)
                .context("Failed to generate embedding for chunk")?;

            let chunk_meta = ChunkMetadata {
                chunk_id: format!("{}-{}", article.id, i),
                document_id: article.id.clone(),
                chunk_index: i as u32,
                total_chunks: chunks.len() as u32,
                document_path: format!("article://{}", article.id),
                document_title: article.title.clone(),
                source_type: article.source_type.clone(),
                source_uri: format!("article://{}", article.id),
                text: chunk_text.clone(),
                token_count: (chunk_text.len() / 4) as u32, // rough estimate
                char_count: chunk_text.len() as u32,
                start_line: None,
                end_line: None,
                start_char: 0,
                end_char: chunk_text.len() as u32,
                heading: None,
                parent_heading: None,
                indexed_at: now.clone(),
                document_modified_at: article.updated_at.clone(),
            };

            chunk_data.push((chunk_meta, embedding));
        }

        let doc_meta = DocumentMetadata {
            id: article.id.clone(),
            title: article.title.clone(),
            path: format!("article://{}", article.id),
            filename: article.title.clone(),
            source_type: article.source_type.clone(),
            source_uri: format!("article://{}", article.id),
            source_version: "1".into(),
            content_type: "text/plain".into(),
            content_hash: String::new(),
            size_bytes: article.content.len() as u64,
            created_at: Some(article.created_at.clone()),
            modified_at: article.updated_at.clone(),
            indexed_at: now,
            last_accessed_at: None,
            document_type: Some("article".into()),
            language: None,
            line_count: article.content.lines().count() as u32,
            word_count: article.content.split_whitespace().count() as u32,
            char_count: article.content.len() as u32,
            chunk_count: chunks.len() as u32,
            status: "active".into(),
            processing_status: "completed".into(),
            access_count: 0,
            is_pinned: false,
        };

        self.vectordb
            .insert_document(&doc_meta, &chunk_data)
            .await?;

        Ok(())
    }
}

/// Split text into chunks with overlap
pub fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();

    if words.len() <= chunk_size {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;

    while start < words.len() {
        let end = (start + chunk_size).min(words.len());
        let chunk = words[start..end].join(" ");
        chunks.push(chunk);

        if end >= words.len() {
            break;
        }

        start += chunk_size - overlap;
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_text_short() {
        let chunks = chunk_text("hello world", 500, 50);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "hello world");
    }

    #[test]
    fn test_chunk_text_with_overlap() {
        let text: String = (0..100)
            .map(|i| format!("word{}", i))
            .collect::<Vec<_>>()
            .join(" ");
        let chunks = chunk_text(&text, 30, 5);
        assert!(chunks.len() > 1);
        // Verify overlap - last 5 words of chunk 0 should appear in chunk 1
        let words0: Vec<&str> = chunks[0].split_whitespace().collect();
        let words1: Vec<&str> = chunks[1].split_whitespace().collect();
        let overlap_words = &words0[words0.len() - 5..];
        let start_words = &words1[..5];
        assert_eq!(overlap_words, start_words);
    }
}
