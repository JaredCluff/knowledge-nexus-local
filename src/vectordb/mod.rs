//! LanceDB vector database wrapper.
//!
//! Provides an embedded vector database for semantic file search.
//! Schema is designed to match Knowledge Nexus metadata for seamless sync.

use anyhow::{Context, Result};
use arrow_array::{
    Array, BooleanArray, Float32Array, RecordBatch, RecordBatchIterator, StringArray, UInt32Array,
    UInt64Array,
};
use arrow_schema::{DataType, Field, Schema};
use futures_util::TryStreamExt;
use lancedb::connect;
use lancedb::query::{ExecutableQuery, QueryBase};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info};

use crate::config;
use crate::embeddings::EMBEDDING_DIM;

const TABLE_NAME: &str = "documents";
const CHUNKS_TABLE: &str = "chunks";

/// Document metadata aligned with Knowledge Nexus schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMetadata {
    // Core identification
    pub id: String,
    pub title: String,
    pub path: String,
    pub filename: String,

    // Source tracking (matches KN source_uri format)
    pub source_type: String,    // "local", "github", "confluence", etc.
    pub source_uri: String,     // e.g., "local:///path/to/file.md"
    pub source_version: String, // File hash or git SHA

    // Content info
    pub content_type: String, // MIME type
    pub content_hash: String, // SHA256 of content
    pub size_bytes: u64,

    // Timestamps (ISO 8601)
    pub created_at: Option<String>,
    pub modified_at: String,
    pub indexed_at: String,
    pub last_accessed_at: Option<String>,

    // Document classification
    pub document_type: Option<String>, // "code", "docs", "config", etc.
    pub language: Option<String>,      // Programming language if code

    // Content metrics
    pub line_count: u32,
    pub word_count: u32,
    pub char_count: u32,

    // Chunk info
    pub chunk_count: u32,

    // Status
    pub status: String,            // "active", "stale", "deleted"
    pub processing_status: String, // "pending", "completed", "failed"

    // Access tracking
    pub access_count: u32,
    pub is_pinned: bool,
}

/// Chunk metadata for vector search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkMetadata {
    // Chunk identification
    pub chunk_id: String,
    pub document_id: String,
    pub chunk_index: u32,
    pub total_chunks: u32,

    // Document reference (denormalized for search)
    pub document_path: String,
    pub document_title: String,
    pub source_type: String,
    pub source_uri: String,

    // Chunk content
    pub text: String,
    pub token_count: u32,
    pub char_count: u32,

    // Position in document
    pub start_line: Option<u32>,
    pub end_line: Option<u32>,
    pub start_char: u32,
    pub end_char: u32,

    // Context
    pub heading: Option<String>,        // Nearest heading/section
    pub parent_heading: Option<String>, // Parent section

    // Timestamps
    pub indexed_at: String,
    pub document_modified_at: String,
}

/// Search result from vector database
#[derive(Debug, Clone)]
pub struct VectorSearchResult {
    pub id: String,
    pub path: String,
    pub title: String,
    pub score: f32,
    pub size_bytes: u64,
    pub modified_at: String,
    pub content_type: String,
    pub source_type: String,
    pub document_type: Option<String>,
    pub chunk_text: Option<String>,
    pub chunk_index: Option<u32>,
}

/// Vector database wrapper
pub struct VectorDB {
    db: lancedb::Connection,
}

impl VectorDB {
    /// Create or open the vector database
    pub async fn new() -> Result<Self> {
        let db_path = config::data_dir().join("vectordb");
        std::fs::create_dir_all(&db_path)?;

        info!("Opening vector database at: {}", db_path.display());

        let db = connect(db_path.to_string_lossy().as_ref())
            .execute()
            .await
            .context("Failed to connect to LanceDB")?;

        let vectordb = Self { db };

        // Ensure tables exist
        vectordb.ensure_tables().await?;

        Ok(vectordb)
    }

    /// Ensure all tables exist
    async fn ensure_tables(&self) -> Result<()> {
        let tables = self.db.table_names().execute().await?;

        if !tables.contains(&TABLE_NAME.to_string()) {
            info!("Creating documents table...");
            self.create_documents_table().await?;
        }

        if !tables.contains(&CHUNKS_TABLE.to_string()) {
            info!("Creating chunks table...");
            self.create_chunks_table().await?;
        }

        Ok(())
    }

    /// Create the documents table (full metadata, no vectors)
    async fn create_documents_table(&self) -> Result<()> {
        let schema = Arc::new(Schema::new(vec![
            // Core identification
            Field::new("id", DataType::Utf8, false),
            Field::new("title", DataType::Utf8, false),
            Field::new("path", DataType::Utf8, false),
            Field::new("filename", DataType::Utf8, false),
            // Source tracking
            Field::new("source_type", DataType::Utf8, false),
            Field::new("source_uri", DataType::Utf8, false),
            Field::new("source_version", DataType::Utf8, false),
            // Content info
            Field::new("content_type", DataType::Utf8, false),
            Field::new("content_hash", DataType::Utf8, false),
            Field::new("size_bytes", DataType::UInt64, false),
            // Timestamps
            Field::new("created_at", DataType::Utf8, true),
            Field::new("modified_at", DataType::Utf8, false),
            Field::new("indexed_at", DataType::Utf8, false),
            Field::new("last_accessed_at", DataType::Utf8, true),
            // Classification
            Field::new("document_type", DataType::Utf8, true),
            Field::new("language", DataType::Utf8, true),
            // Metrics
            Field::new("line_count", DataType::UInt32, false),
            Field::new("word_count", DataType::UInt32, false),
            Field::new("char_count", DataType::UInt32, false),
            Field::new("chunk_count", DataType::UInt32, false),
            // Status
            Field::new("status", DataType::Utf8, false),
            Field::new("processing_status", DataType::Utf8, false),
            // Access tracking
            Field::new("access_count", DataType::UInt32, false),
            Field::new("is_pinned", DataType::Boolean, false),
        ]));

        let batch = RecordBatch::new_empty(schema.clone());
        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);

        self.db
            .create_table(TABLE_NAME, Box::new(batches))
            .execute()
            .await?;

        info!("Documents table created");
        Ok(())
    }

    /// Create the chunks table (with vectors for search)
    async fn create_chunks_table(&self) -> Result<()> {
        let schema = Arc::new(Schema::new(vec![
            // Chunk identification
            Field::new("chunk_id", DataType::Utf8, false),
            Field::new("document_id", DataType::Utf8, false),
            Field::new("chunk_index", DataType::UInt32, false),
            Field::new("total_chunks", DataType::UInt32, false),
            // Document reference (denormalized)
            Field::new("document_path", DataType::Utf8, false),
            Field::new("document_title", DataType::Utf8, false),
            Field::new("source_type", DataType::Utf8, false),
            Field::new("source_uri", DataType::Utf8, false),
            // Embedding vector
            Field::new(
                "embedding",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    EMBEDDING_DIM as i32,
                ),
                false,
            ),
            // Chunk content
            Field::new("text", DataType::Utf8, false),
            Field::new("token_count", DataType::UInt32, false),
            Field::new("char_count", DataType::UInt32, false),
            // Position
            Field::new("start_line", DataType::UInt32, true),
            Field::new("end_line", DataType::UInt32, true),
            Field::new("start_char", DataType::UInt32, false),
            Field::new("end_char", DataType::UInt32, false),
            // Context
            Field::new("heading", DataType::Utf8, true),
            Field::new("parent_heading", DataType::Utf8, true),
            // Timestamps
            Field::new("indexed_at", DataType::Utf8, false),
            Field::new("document_modified_at", DataType::Utf8, false),
        ]));

        let batch = RecordBatch::new_empty(schema.clone());
        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);

        self.db
            .create_table(CHUNKS_TABLE, Box::new(batches))
            .execute()
            .await?;

        info!("Chunks table created");
        Ok(())
    }

    /// Insert a document with its chunks
    pub async fn insert_document(
        &self,
        metadata: &DocumentMetadata,
        chunks: &[(ChunkMetadata, Vec<f32>)],
    ) -> Result<()> {
        // Delete existing document if present (log but don't fail on delete errors)
        if let Err(e) = self.delete_document(&metadata.id).await {
            tracing::debug!("No existing document to delete or delete failed: {}", e);
        }

        // Insert document metadata
        self.insert_document_metadata(metadata).await?;

        // Insert all chunks
        for (chunk_meta, embedding) in chunks {
            self.insert_chunk(chunk_meta, embedding).await?;
        }

        debug!(
            "Inserted document: {} with {} chunks",
            metadata.path,
            chunks.len()
        );
        Ok(())
    }

    /// Insert document metadata only
    async fn insert_document_metadata(&self, meta: &DocumentMetadata) -> Result<()> {
        let schema = self.documents_schema();

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![meta.id.as_str()])),
                Arc::new(StringArray::from(vec![meta.title.as_str()])),
                Arc::new(StringArray::from(vec![meta.path.as_str()])),
                Arc::new(StringArray::from(vec![meta.filename.as_str()])),
                Arc::new(StringArray::from(vec![meta.source_type.as_str()])),
                Arc::new(StringArray::from(vec![meta.source_uri.as_str()])),
                Arc::new(StringArray::from(vec![meta.source_version.as_str()])),
                Arc::new(StringArray::from(vec![meta.content_type.as_str()])),
                Arc::new(StringArray::from(vec![meta.content_hash.as_str()])),
                Arc::new(UInt64Array::from(vec![meta.size_bytes])),
                Arc::new(StringArray::from(vec![meta.created_at.as_deref()])),
                Arc::new(StringArray::from(vec![meta.modified_at.as_str()])),
                Arc::new(StringArray::from(vec![meta.indexed_at.as_str()])),
                Arc::new(StringArray::from(vec![meta.last_accessed_at.as_deref()])),
                Arc::new(StringArray::from(vec![meta.document_type.as_deref()])),
                Arc::new(StringArray::from(vec![meta.language.as_deref()])),
                Arc::new(UInt32Array::from(vec![meta.line_count])),
                Arc::new(UInt32Array::from(vec![meta.word_count])),
                Arc::new(UInt32Array::from(vec![meta.char_count])),
                Arc::new(UInt32Array::from(vec![meta.chunk_count])),
                Arc::new(StringArray::from(vec![meta.status.as_str()])),
                Arc::new(StringArray::from(vec![meta.processing_status.as_str()])),
                Arc::new(UInt32Array::from(vec![meta.access_count])),
                Arc::new(BooleanArray::from(vec![meta.is_pinned])),
            ],
        )?;

        let table = self.db.open_table(TABLE_NAME).execute().await?;
        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
        table.add(Box::new(batches)).execute().await?;

        Ok(())
    }

    /// Insert a single chunk
    async fn insert_chunk(&self, meta: &ChunkMetadata, embedding: &[f32]) -> Result<()> {
        let schema = self.chunks_schema();
        let embedding_array = create_embedding_array(embedding)?;

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec![meta.chunk_id.as_str()])),
                Arc::new(StringArray::from(vec![meta.document_id.as_str()])),
                Arc::new(UInt32Array::from(vec![meta.chunk_index])),
                Arc::new(UInt32Array::from(vec![meta.total_chunks])),
                Arc::new(StringArray::from(vec![meta.document_path.as_str()])),
                Arc::new(StringArray::from(vec![meta.document_title.as_str()])),
                Arc::new(StringArray::from(vec![meta.source_type.as_str()])),
                Arc::new(StringArray::from(vec![meta.source_uri.as_str()])),
                Arc::new(embedding_array),
                Arc::new(StringArray::from(vec![meta.text.as_str()])),
                Arc::new(UInt32Array::from(vec![meta.token_count])),
                Arc::new(UInt32Array::from(vec![meta.char_count])),
                Arc::new(UInt32Array::from(vec![meta.start_line])),
                Arc::new(UInt32Array::from(vec![meta.end_line])),
                Arc::new(UInt32Array::from(vec![meta.start_char])),
                Arc::new(UInt32Array::from(vec![meta.end_char])),
                Arc::new(StringArray::from(vec![meta.heading.as_deref()])),
                Arc::new(StringArray::from(vec![meta.parent_heading.as_deref()])),
                Arc::new(StringArray::from(vec![meta.indexed_at.as_str()])),
                Arc::new(StringArray::from(vec![meta.document_modified_at.as_str()])),
            ],
        )?;

        let table = self.db.open_table(CHUNKS_TABLE).execute().await?;
        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
        table.add(Box::new(batches)).execute().await?;

        Ok(())
    }

    /// Backwards-compatible insert for simple file indexing
    pub async fn insert(
        &self,
        path: &str,
        embedding: &[f32],
        size_bytes: u64,
        modified_at: &str,
    ) -> Result<()> {
        let id = generate_id(path);
        let filename = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let now = chrono::Utc::now().to_rfc3339();

        // Create minimal document metadata
        let doc_meta = DocumentMetadata {
            id: id.clone(),
            title: filename.clone(),
            path: path.to_string(),
            filename: filename.clone(),
            source_type: "local".to_string(),
            source_uri: format!("local://{}", path),
            source_version: generate_id(&format!("{}{}", path, modified_at)),
            content_type: mime_guess::from_path(path)
                .first_or_octet_stream()
                .to_string(),
            content_hash: "".to_string(), // Would need to compute from content
            size_bytes,
            created_at: None,
            modified_at: modified_at.to_string(),
            indexed_at: now.clone(),
            last_accessed_at: None,
            document_type: detect_document_type(path),
            language: detect_language(path),
            line_count: 0,
            word_count: 0,
            char_count: 0,
            chunk_count: 1,
            status: "active".to_string(),
            processing_status: "completed".to_string(),
            access_count: 0,
            is_pinned: false,
        };

        // Create single chunk
        let chunk_meta = ChunkMetadata {
            chunk_id: format!("{}-0", id),
            document_id: id,
            chunk_index: 0,
            total_chunks: 1,
            document_path: path.to_string(),
            document_title: filename,
            source_type: "local".to_string(),
            source_uri: format!("local://{}", path),
            text: "".to_string(), // Not storing full text in legacy mode
            token_count: 0,
            char_count: 0,
            start_line: None,
            end_line: None,
            start_char: 0,
            end_char: 0,
            heading: None,
            parent_heading: None,
            indexed_at: now.clone(),
            document_modified_at: modified_at.to_string(),
        };

        self.insert_document(&doc_meta, &[(chunk_meta, embedding.to_vec())])
            .await
    }

    /// Search for similar documents/chunks
    pub async fn search(
        &self,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<VectorSearchResult>> {
        let table = self.db.open_table(CHUNKS_TABLE).execute().await?;

        let results = table
            .vector_search(query_embedding.to_vec())?
            .limit(limit)
            .execute()
            .await?;

        let mut search_results = Vec::new();
        let batches: Vec<RecordBatch> = results.try_collect().await?;

        for batch in batches {
            let chunk_ids = batch
                .column_by_name("chunk_id")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let paths = batch
                .column_by_name("document_path")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let titles = batch
                .column_by_name("document_title")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let source_types = batch
                .column_by_name("source_type")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let texts = batch
                .column_by_name("text")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let chunk_indices = batch
                .column_by_name("chunk_index")
                .and_then(|c| c.as_any().downcast_ref::<UInt32Array>());
            let modified_dates = batch
                .column_by_name("document_modified_at")
                .and_then(|c| c.as_any().downcast_ref::<StringArray>());
            let distances = batch
                .column_by_name("_distance")
                .and_then(|c| c.as_any().downcast_ref::<Float32Array>());

            if let (
                Some(chunk_ids),
                Some(paths),
                Some(titles),
                Some(source_types),
                Some(distances),
            ) = (chunk_ids, paths, titles, source_types, distances)
            {
                for i in 0..batch.num_rows() {
                    let distance = distances.value(i);
                    let score = 1.0 - distance;

                    search_results.push(VectorSearchResult {
                        id: chunk_ids.value(i).to_string(),
                        path: paths.value(i).to_string(),
                        title: titles.value(i).to_string(),
                        score,
                        size_bytes: 0, // Would need to join with documents table
                        modified_at: modified_dates
                            .map(|m| m.value(i).to_string())
                            .unwrap_or_default(),
                        content_type: "".to_string(),
                        source_type: source_types.value(i).to_string(),
                        document_type: None,
                        chunk_text: texts.map(|t| t.value(i).to_string()),
                        chunk_index: chunk_indices.map(|c| c.value(i)),
                    });
                }
            }
        }

        Ok(search_results)
    }

    /// Delete a document and all its chunks
    pub async fn delete_document(&self, document_id: &str) -> Result<()> {
        // Validate document_id to prevent SQL injection
        // IDs are either hex strings (from SHA256 hash) or UUIDs
        if !document_id
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == '-')
        {
            anyhow::bail!("Invalid document_id format: must be hex string or UUID");
        }

        // Delete from documents table
        let docs_table = self.db.open_table(TABLE_NAME).execute().await?;
        if let Err(e) = docs_table.delete(&format!("id = '{}'", document_id)).await {
            tracing::warn!("Failed to delete document {}: {}", document_id, e);
        }

        // Delete chunks
        let chunks_table = self.db.open_table(CHUNKS_TABLE).execute().await?;
        if let Err(e) = chunks_table
            .delete(&format!("document_id = '{}'", document_id))
            .await
        {
            tracing::warn!(
                "Failed to delete chunks for document {}: {}",
                document_id,
                e
            );
        }

        debug!("Deleted document: {}", document_id);
        Ok(())
    }

    /// Delete by path (backwards compatible)
    pub async fn delete(&self, path: &str) -> Result<()> {
        let id = generate_id(path);
        self.delete_document(&id).await
    }

    /// Clear all entries
    pub async fn clear(&self) -> Result<()> {
        info!("Clearing vector database...");

        // Drop tables (log errors but continue - tables may not exist)
        if let Err(e) = self.db.drop_table(TABLE_NAME).await {
            tracing::debug!("Could not drop documents table (may not exist): {}", e);
        }
        if let Err(e) = self.db.drop_table(CHUNKS_TABLE).await {
            tracing::debug!("Could not drop chunks table (may not exist): {}", e);
        }
        self.ensure_tables().await?;

        Ok(())
    }

    /// Get count of indexed documents
    pub async fn count(&self) -> Result<usize> {
        let table = self.db.open_table(TABLE_NAME).execute().await?;
        let count = table.count_rows(None).await?;
        Ok(count)
    }

    /// Get count of indexed chunks
    #[allow(dead_code)]
    pub async fn chunk_count(&self) -> Result<usize> {
        let table = self.db.open_table(CHUNKS_TABLE).execute().await?;
        let count = table.count_rows(None).await?;
        Ok(count)
    }

    fn documents_schema(&self) -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("title", DataType::Utf8, false),
            Field::new("path", DataType::Utf8, false),
            Field::new("filename", DataType::Utf8, false),
            Field::new("source_type", DataType::Utf8, false),
            Field::new("source_uri", DataType::Utf8, false),
            Field::new("source_version", DataType::Utf8, false),
            Field::new("content_type", DataType::Utf8, false),
            Field::new("content_hash", DataType::Utf8, false),
            Field::new("size_bytes", DataType::UInt64, false),
            Field::new("created_at", DataType::Utf8, true),
            Field::new("modified_at", DataType::Utf8, false),
            Field::new("indexed_at", DataType::Utf8, false),
            Field::new("last_accessed_at", DataType::Utf8, true),
            Field::new("document_type", DataType::Utf8, true),
            Field::new("language", DataType::Utf8, true),
            Field::new("line_count", DataType::UInt32, false),
            Field::new("word_count", DataType::UInt32, false),
            Field::new("char_count", DataType::UInt32, false),
            Field::new("chunk_count", DataType::UInt32, false),
            Field::new("status", DataType::Utf8, false),
            Field::new("processing_status", DataType::Utf8, false),
            Field::new("access_count", DataType::UInt32, false),
            Field::new("is_pinned", DataType::Boolean, false),
        ]))
    }

    fn chunks_schema(&self) -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("chunk_id", DataType::Utf8, false),
            Field::new("document_id", DataType::Utf8, false),
            Field::new("chunk_index", DataType::UInt32, false),
            Field::new("total_chunks", DataType::UInt32, false),
            Field::new("document_path", DataType::Utf8, false),
            Field::new("document_title", DataType::Utf8, false),
            Field::new("source_type", DataType::Utf8, false),
            Field::new("source_uri", DataType::Utf8, false),
            Field::new(
                "embedding",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    EMBEDDING_DIM as i32,
                ),
                false,
            ),
            Field::new("text", DataType::Utf8, false),
            Field::new("token_count", DataType::UInt32, false),
            Field::new("char_count", DataType::UInt32, false),
            Field::new("start_line", DataType::UInt32, true),
            Field::new("end_line", DataType::UInt32, true),
            Field::new("start_char", DataType::UInt32, false),
            Field::new("end_char", DataType::UInt32, false),
            Field::new("heading", DataType::Utf8, true),
            Field::new("parent_heading", DataType::Utf8, true),
            Field::new("indexed_at", DataType::Utf8, false),
            Field::new("document_modified_at", DataType::Utf8, false),
        ]))
    }
}

/// Generate a unique ID from a file path
fn generate_id(path: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(path.as_bytes());
    let hash = hasher.finalize();
    hex::encode(&hash[..16])
}

/// Create a FixedSizeList array for embeddings
fn create_embedding_array(embedding: &[f32]) -> Result<arrow_array::FixedSizeListArray> {
    use arrow_array::builder::{FixedSizeListBuilder, Float32Builder};

    let mut builder = FixedSizeListBuilder::new(Float32Builder::new(), EMBEDDING_DIM as i32);
    let values = builder.values();
    for &v in embedding {
        values.append_value(v);
    }
    builder.append(true);

    Ok(builder.finish())
}

/// Detect document type from path
fn detect_document_type(path: &str) -> Option<String> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());

    match ext.as_deref() {
        Some("md" | "markdown" | "rst" | "txt") => Some("docs".to_string()),
        Some("rs" | "py" | "js" | "ts" | "go" | "java" | "c" | "cpp" | "h") => {
            Some("code".to_string())
        }
        Some("yaml" | "yml" | "json" | "toml" | "ini" | "cfg" | "conf") => {
            Some("config".to_string())
        }
        Some("sql") => Some("database".to_string()),
        Some("sh" | "bash" | "zsh" | "ps1") => Some("script".to_string()),
        _ => None,
    }
}

/// Detect programming language from path
fn detect_language(path: &str) -> Option<String> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());

    match ext.as_deref() {
        Some("rs") => Some("rust".to_string()),
        Some("py") => Some("python".to_string()),
        Some("js") => Some("javascript".to_string()),
        Some("ts") => Some("typescript".to_string()),
        Some("go") => Some("go".to_string()),
        Some("java") => Some("java".to_string()),
        Some("c") => Some("c".to_string()),
        Some("cpp" | "cc" | "cxx") => Some("cpp".to_string()),
        Some("h" | "hpp") => Some("c-header".to_string()),
        Some("rb") => Some("ruby".to_string()),
        Some("php") => Some("php".to_string()),
        Some("swift") => Some("swift".to_string()),
        Some("kt" | "kts") => Some("kotlin".to_string()),
        Some("cs") => Some("csharp".to_string()),
        Some("sql") => Some("sql".to_string()),
        Some("sh" | "bash") => Some("bash".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_vectordb_operations() {
        let db = VectorDB::new().await.unwrap();

        // Insert a test record
        let embedding = vec![0.1f32; EMBEDDING_DIM];
        db.insert("/test/file.txt", &embedding, 100, "2024-01-01T00:00:00Z")
            .await
            .unwrap();

        // Search
        let results = db.search(&embedding, 10).await.unwrap();
        assert!(!results.is_empty());

        // Delete
        db.delete("/test/file.txt").await.unwrap();
    }
}
