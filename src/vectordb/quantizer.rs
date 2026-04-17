//! Pluggable vector quantizer abstractions for LanceDB-backed semantic search.
//!
//! Provides a `VectorQuantizer` trait with concrete implementations for IVF-PQ,
//! IVF-HNSW-SQ (int8), and a TurboQuant stub.  A `QuantizerRegistry` maps
//! version strings to the appropriate implementation at runtime.

use anyhow::{anyhow, Result};
use arrow_array::RecordBatch;
use async_trait::async_trait;
use futures_util::TryStreamExt;
use lancedb::index::Index;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::Table;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// QuantizerFilter
// ---------------------------------------------------------------------------

/// Optional pre-search filter applied to vector queries.
#[derive(Debug, Clone, Default)]
pub struct QuantizerFilter {
    /// SQL-style WHERE clause fragment, e.g. `"source_type = 'local'"`.
    pub where_clause: Option<String>,
    /// Maximum number of results to return.
    pub limit: Option<usize>,
}

// ---------------------------------------------------------------------------
// VectorQuantizer trait
// ---------------------------------------------------------------------------

/// Trait implemented by every quantizer strategy.
///
/// Each implementation wraps a particular LanceDB index type and exposes a
/// uniform interface for index creation and vector search.
#[async_trait]
pub trait VectorQuantizer: Send + Sync {
    /// Canonical version string used to identify this quantizer, e.g. `"ivf_pq_v1"`.
    fn version(&self) -> &'static str;

    /// Build (or rebuild) the vector index on `table`.
    ///
    /// Implementations **must** skip index creation when the table has fewer
    /// rows than needed for the chosen algorithm (e.g. < 256 for IVF-PQ).
    async fn build_index(&self, table: &Table) -> Result<()>;

    /// Run a vector similarity search against `table`.
    ///
    /// Returns the raw `RecordBatch` output from LanceDB so that the caller
    /// can parse columns in its own way.
    async fn search(
        &self,
        table: &Table,
        query: &[f32],
        filter: &QuantizerFilter,
    ) -> Result<Vec<RecordBatch>>;
}

// ---------------------------------------------------------------------------
// Task 2 — IvfPqQuantizer
// ---------------------------------------------------------------------------

/// Wraps LanceDB's IVF-PQ vector index.
///
/// IVF-PQ gives excellent recall/speed trade-offs for large datasets.  Index
/// creation is skipped when the table has fewer than 256 rows because LanceDB
/// requires at least that many training points.
pub struct IvfPqQuantizer;

#[async_trait]
impl VectorQuantizer for IvfPqQuantizer {
    fn version(&self) -> &'static str {
        "ivf_pq_v1"
    }

    async fn build_index(&self, table: &Table) -> Result<()> {
        let row_count = table.count_rows(None).await?;
        if row_count < 256 {
            tracing::warn!(
                "IvfPqQuantizer: only {} rows, skipping index creation (need >= 256)",
                row_count
            );
            return Ok(());
        }

        table
            .create_index(&["embedding"], Index::IvfPq(Default::default()))
            .execute()
            .await
            .map_err(|e| anyhow!("IvfPqQuantizer: failed to build index: {}", e))?;

        Ok(())
    }

    async fn search(
        &self,
        table: &Table,
        query: &[f32],
        filter: &QuantizerFilter,
    ) -> Result<Vec<RecordBatch>> {
        let limit = filter.limit.unwrap_or(10);
        let mut builder = table
            .vector_search(query.to_vec())
            .map_err(|e| anyhow!("IvfPqQuantizer: vector_search init failed: {}", e))?
            .limit(limit);

        if let Some(ref clause) = filter.where_clause {
            builder = builder.only_if(clause.clone());
        }

        let stream = builder
            .execute()
            .await
            .map_err(|e| anyhow!("IvfPqQuantizer: search execute failed: {}", e))?;

        let batches: Vec<RecordBatch> = stream
            .try_collect()
            .await
            .map_err(|e| anyhow!("IvfPqQuantizer: collecting results failed: {}", e))?;

        Ok(batches)
    }
}

// ---------------------------------------------------------------------------
// Task 3 — Int8Quantizer
// ---------------------------------------------------------------------------

/// Wraps LanceDB's IVF-HNSW-SQ (scalar-quantized) index.
///
/// Scalar quantization compresses 32-bit floats to 8-bit integers, cutting
/// memory usage by ~4× while preserving most of the recall.  Index creation
/// is skipped when the table has fewer than 256 rows.
pub struct Int8Quantizer;

#[async_trait]
impl VectorQuantizer for Int8Quantizer {
    fn version(&self) -> &'static str {
        "int8_v1"
    }

    async fn build_index(&self, table: &Table) -> Result<()> {
        let row_count = table.count_rows(None).await?;
        if row_count < 256 {
            tracing::warn!(
                "Int8Quantizer: only {} rows, skipping index creation (need >= 256)",
                row_count
            );
            return Ok(());
        }

        table
            .create_index(&["embedding"], Index::IvfHnswSq(Default::default()))
            .execute()
            .await
            .map_err(|e| anyhow!("Int8Quantizer: failed to build index: {}", e))?;

        Ok(())
    }

    async fn search(
        &self,
        table: &Table,
        query: &[f32],
        filter: &QuantizerFilter,
    ) -> Result<Vec<RecordBatch>> {
        let limit = filter.limit.unwrap_or(10);
        let mut builder = table
            .vector_search(query.to_vec())
            .map_err(|e| anyhow!("Int8Quantizer: vector_search init failed: {}", e))?
            .limit(limit);

        if let Some(ref clause) = filter.where_clause {
            builder = builder.only_if(clause.clone());
        }

        let stream = builder
            .execute()
            .await
            .map_err(|e| anyhow!("Int8Quantizer: search execute failed: {}", e))?;

        let batches: Vec<RecordBatch> = stream
            .try_collect()
            .await
            .map_err(|e| anyhow!("Int8Quantizer: collecting results failed: {}", e))?;

        Ok(batches)
    }
}

// ---------------------------------------------------------------------------
// Task 4 — TurboQuantQuantizer stub
// ---------------------------------------------------------------------------

/// Stub for the upcoming TurboQuant quantizer.
///
/// All methods return an error explaining that TurboQuant is not yet available.
/// Enable this implementation with the `turboquant` Cargo feature once the
/// upstream crate is integrated.
pub struct TurboQuantQuantizer;

#[async_trait]
impl VectorQuantizer for TurboQuantQuantizer {
    fn version(&self) -> &'static str {
        "turboquant_v1"
    }

    async fn build_index(&self, _table: &Table) -> Result<()> {
        Err(anyhow!(
            "TurboQuantQuantizer: not yet available — enable the `turboquant` feature once the upstream crate is integrated"
        ))
    }

    async fn search(
        &self,
        _table: &Table,
        _query: &[f32],
        _filter: &QuantizerFilter,
    ) -> Result<Vec<RecordBatch>> {
        Err(anyhow!(
            "TurboQuantQuantizer: not yet available — enable the `turboquant` feature once the upstream crate is integrated"
        ))
    }
}

// ---------------------------------------------------------------------------
// Task 5 — QuantizerRegistry
// ---------------------------------------------------------------------------

/// Maps version strings to concrete `VectorQuantizer` implementations.
///
/// # Example
/// ```rust,ignore
/// let registry = QuantizerRegistry::new();
/// let q = registry.resolve("ivf_pq_v1")?;
/// q.build_index(&table).await?;
/// ```
pub struct QuantizerRegistry {
    entries: Vec<(&'static str, Arc<dyn VectorQuantizer>)>,
}

impl QuantizerRegistry {
    /// Create a new registry pre-populated with all built-in quantizers.
    pub fn new() -> Self {
        let entries: Vec<(&'static str, Arc<dyn VectorQuantizer>)> = vec![
            ("ivf_pq_v1", Arc::new(IvfPqQuantizer)),
            ("int8_v1", Arc::new(Int8Quantizer)),
            ("turboquant_v1", Arc::new(TurboQuantQuantizer)),
        ];
        Self { entries }
    }

    /// Look up a quantizer by its version string.
    ///
    /// Returns an error when `version` is not registered.
    pub fn resolve(&self, version: &str) -> Result<Arc<dyn VectorQuantizer>> {
        self.entries
            .iter()
            .find(|(v, _)| *v == version)
            .map(|(_, q)| Arc::clone(q))
            .ok_or_else(|| {
                anyhow!(
                    "Unknown quantizer version '{}'. Supported versions: {}",
                    version,
                    self.supported_versions().join(", ")
                )
            })
    }

    /// Returns the list of all registered version strings.
    pub fn supported_versions(&self) -> Vec<&'static str> {
        self.entries.iter().map(|(v, _)| *v).collect()
    }
}

impl Default for QuantizerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{
        builder::{FixedSizeListBuilder, Float32Builder},
        RecordBatchIterator, StringArray, UInt32Array,
    };
    use arrow_schema::{DataType, Field, Schema};
    use lancedb::connect;
    use std::sync::Arc;
    use tempfile::TempDir;

    use crate::embeddings::EMBEDDING_DIM;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn chunks_schema() -> Arc<Schema> {
        Arc::new(Schema::new(vec![
            Field::new("chunk_id", DataType::Utf8, false),
            Field::new("document_id", DataType::Utf8, false),
            Field::new(
                "embedding",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    EMBEDDING_DIM as i32,
                ),
                false,
            ),
            Field::new("document_path", DataType::Utf8, false),
            Field::new("document_title", DataType::Utf8, false),
            Field::new("source_type", DataType::Utf8, false),
            Field::new("text", DataType::Utf8, false),
            Field::new("chunk_index", DataType::UInt32, false),
            Field::new("document_modified_at", DataType::Utf8, false),
        ]))
    }

    fn make_embedding_array(n: usize, value: f32) -> Arc<dyn arrow_array::Array> {
        let mut builder =
            FixedSizeListBuilder::new(Float32Builder::new(), EMBEDDING_DIM as i32);
        for _ in 0..n {
            let vals = builder.values();
            for _ in 0..EMBEDDING_DIM {
                vals.append_value(value);
            }
            builder.append(true);
        }
        Arc::new(builder.finish())
    }

    async fn create_test_table(dir: &TempDir, n_rows: usize) -> lancedb::Table {
        let schema = chunks_schema();
        let db = connect(dir.path().to_string_lossy().as_ref())
            .execute()
            .await
            .unwrap();

        let chunk_ids: Vec<String> = (0..n_rows).map(|i| format!("chunk-{}", i)).collect();
        let doc_ids: Vec<String> = (0..n_rows).map(|i| format!("doc-{}", i)).collect();
        let paths: Vec<String> = (0..n_rows)
            .map(|i| format!("/path/file{}.txt", i))
            .collect();
        let titles: Vec<String> = (0..n_rows).map(|i| format!("Title {}", i)).collect();
        let texts: Vec<String> = (0..n_rows).map(|i| format!("text content {}", i)).collect();
        let dates: Vec<String> = (0..n_rows).map(|_| "2024-01-01T00:00:00Z".to_string()).collect();
        let source_types: Vec<&str> = (0..n_rows).map(|_| "local").collect();
        let chunk_indices: Vec<u32> = (0..n_rows as u32).collect();

        let embedding_array = make_embedding_array(n_rows, 0.1);

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(
                    chunk_ids.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    doc_ids.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                )),
                embedding_array,
                Arc::new(StringArray::from(
                    paths.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(
                    titles.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(source_types)),
                Arc::new(StringArray::from(
                    texts.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                )),
                Arc::new(UInt32Array::from(chunk_indices)),
                Arc::new(StringArray::from(
                    dates.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                )),
            ],
        )
        .unwrap();

        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
        db.create_table("chunks", Box::new(batches))
            .execute()
            .await
            .unwrap()
    }

    // -----------------------------------------------------------------------
    // Task 1: Object safety
    // -----------------------------------------------------------------------

    #[test]
    fn test_vector_quantizer_is_object_safe() {
        // If VectorQuantizer were not object-safe this line would not compile.
        let _: Option<Arc<dyn VectorQuantizer>> = None;
    }

    // -----------------------------------------------------------------------
    // Task 2: IvfPqQuantizer
    // -----------------------------------------------------------------------

    #[test]
    fn test_ivf_pq_version() {
        let q = IvfPqQuantizer;
        assert_eq!(q.version(), "ivf_pq_v1");
    }

    #[tokio::test]
    async fn test_ivf_pq_search_without_index() {
        let dir = TempDir::new().unwrap();
        let table = create_test_table(&dir, 10).await;

        let q = IvfPqQuantizer;
        // Build index should succeed (skipped due to < 256 rows).
        q.build_index(&table).await.unwrap();

        // Search should still work via brute-force scan.
        let query = vec![0.1f32; EMBEDDING_DIM];
        let filter = QuantizerFilter {
            limit: Some(5),
            ..Default::default()
        };
        let results = q.search(&table, &query, &filter).await.unwrap();
        let total_rows: usize = results.iter().map(|b| b.num_rows()).sum();
        assert!(total_rows > 0, "expected at least one result");
    }

    #[tokio::test]
    async fn test_ivf_pq_search_with_where_clause() {
        let dir = TempDir::new().unwrap();
        let schema = chunks_schema();
        let db = connect(dir.path().to_string_lossy().as_ref())
            .execute()
            .await
            .unwrap();

        // Insert 6 rows: 3 with source_type "local", 3 with "github"
        let embedding_array = make_embedding_array(6, 0.1);
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["c0", "c1", "c2", "c3", "c4", "c5"])),
                Arc::new(StringArray::from(vec!["d0", "d1", "d2", "d3", "d4", "d5"])),
                embedding_array,
                Arc::new(StringArray::from(vec!["/a", "/b", "/c", "/d", "/e", "/f"])),
                Arc::new(StringArray::from(vec!["A", "B", "C", "D", "E", "F"])),
                Arc::new(StringArray::from(vec![
                    "local", "local", "local", "github", "github", "github",
                ])),
                Arc::new(StringArray::from(vec!["t0", "t1", "t2", "t3", "t4", "t5"])),
                Arc::new(UInt32Array::from(vec![0u32, 1, 2, 3, 4, 5])),
                Arc::new(StringArray::from(vec![
                    "2024-01-01T00:00:00Z",
                    "2024-01-01T00:00:00Z",
                    "2024-01-01T00:00:00Z",
                    "2024-01-01T00:00:00Z",
                    "2024-01-01T00:00:00Z",
                    "2024-01-01T00:00:00Z",
                ])),
            ],
        )
        .unwrap();

        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let table = db
            .create_table("chunks", Box::new(batches))
            .execute()
            .await
            .unwrap();

        let q = IvfPqQuantizer;
        let query = vec![0.1f32; EMBEDDING_DIM];
        let filter = QuantizerFilter {
            limit: Some(10),
            where_clause: Some("source_type = 'github'".to_string()),
        };
        let results = q.search(&table, &query, &filter).await.unwrap();
        let total_rows: usize = results.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total_rows, 3, "expected exactly 3 'github' results");

        // Verify all returned rows have source_type = "github"
        for batch in &results {
            let source_types = batch
                .column_by_name("source_type")
                .unwrap()
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap();
            for i in 0..batch.num_rows() {
                assert_eq!(source_types.value(i), "github");
            }
        }
    }

    // -----------------------------------------------------------------------
    // Task 3: Int8Quantizer
    // -----------------------------------------------------------------------

    #[test]
    fn test_int8_version() {
        let q = Int8Quantizer;
        assert_eq!(q.version(), "int8_v1");
    }

    #[tokio::test]
    async fn test_int8_search_without_index() {
        let dir = TempDir::new().unwrap();
        let table = create_test_table(&dir, 10).await;

        let q = Int8Quantizer;
        q.build_index(&table).await.unwrap();

        let query = vec![0.1f32; EMBEDDING_DIM];
        let filter = QuantizerFilter {
            limit: Some(5),
            ..Default::default()
        };
        let results = q.search(&table, &query, &filter).await.unwrap();
        let total_rows: usize = results.iter().map(|b| b.num_rows()).sum();
        assert!(total_rows > 0, "expected at least one result");
    }

    // -----------------------------------------------------------------------
    // Task 4: TurboQuantQuantizer stub
    // -----------------------------------------------------------------------

    #[test]
    fn test_turboquant_stub_version() {
        let q = TurboQuantQuantizer;
        assert_eq!(q.version(), "turboquant_v1");
    }

    #[tokio::test]
    async fn test_turboquant_stub_build_index_errors() {
        let dir = TempDir::new().unwrap();
        let table = create_test_table(&dir, 10).await;

        let q = TurboQuantQuantizer;
        let err = q.build_index(&table).await.unwrap_err();
        assert!(
            err.to_string().contains("not yet available"),
            "expected 'not yet available' in error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_turboquant_stub_search_errors() {
        let dir = TempDir::new().unwrap();
        let table = create_test_table(&dir, 10).await;

        let q = TurboQuantQuantizer;
        let query = vec![0.1f32; EMBEDDING_DIM];
        let filter = QuantizerFilter::default();
        let err = q.search(&table, &query, &filter).await.unwrap_err();
        assert!(
            err.to_string().contains("not yet available"),
            "expected 'not yet available' in error, got: {}",
            err
        );
    }

    // -----------------------------------------------------------------------
    // Task 5: QuantizerRegistry
    // -----------------------------------------------------------------------

    #[test]
    fn test_registry_resolves_ivf_pq() {
        let registry = QuantizerRegistry::new();
        let q = registry.resolve("ivf_pq_v1").unwrap();
        assert_eq!(q.version(), "ivf_pq_v1");
    }

    #[test]
    fn test_registry_resolves_int8() {
        let registry = QuantizerRegistry::new();
        let q = registry.resolve("int8_v1").unwrap();
        assert_eq!(q.version(), "int8_v1");
    }

    #[test]
    fn test_registry_resolves_turboquant() {
        let registry = QuantizerRegistry::new();
        let q = registry.resolve("turboquant_v1").unwrap();
        assert_eq!(q.version(), "turboquant_v1");
    }

    #[test]
    fn test_registry_unknown_version_errors() {
        let registry = QuantizerRegistry::new();
        match registry.resolve("nonexistent") {
            Ok(_) => panic!("expected an error for unknown version, got Ok"),
            Err(e) => {
                assert!(
                    e.to_string().contains("Unknown quantizer"),
                    "expected 'Unknown quantizer' in error, got: {}",
                    e
                );
            }
        }
    }

    #[test]
    fn test_registry_supported_versions() {
        let registry = QuantizerRegistry::new();
        let versions = registry.supported_versions();
        assert!(versions.contains(&"ivf_pq_v1"));
        assert!(versions.contains(&"int8_v1"));
        assert!(versions.contains(&"turboquant_v1"));
        assert_eq!(versions.len(), 3);
    }
}
