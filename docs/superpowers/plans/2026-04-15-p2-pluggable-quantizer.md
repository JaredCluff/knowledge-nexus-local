# P2: Pluggable VectorQuantizer Trait — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Introduce a `VectorQuantizer` trait that abstracts LanceDB's index strategy, ship IVF-PQ and Int8-SQ implementations, stub TurboQuant behind a feature gate, and wire quantizer selection into the CLI via `reindex --quantizer`.

**Architecture:** `VectorDB` gains an `Arc<dyn VectorQuantizer>` field. Each quantizer impl wraps a LanceDB index strategy (`IvfPq`, `IvfHnswSq`) or a custom codec (TurboQuant, P5). The active quantizer is resolved at startup from `knowledge_store.quantizer_version` via a `QuantizerRegistry`. The `reindex` CLI command rebuilds the LanceDB index with the selected quantizer and updates the store's `quantizer_version` in SurrealDB.

**Tech Stack:** Rust, LanceDB 0.13 (`lancedb::index::Index`), Arrow 52, async-trait, SurrealDB 2.x

---

## File Structure

| Action | Path | Responsibility |
|--------|------|----------------|
| Create | `src/vectordb/quantizer.rs` | `VectorQuantizer` trait, `QuantizerFilter`, `QuantizerRegistry`, `IvfPqQuantizer`, `Int8Quantizer`, `TurboQuantQuantizer` stub |
| Modify | `src/vectordb/mod.rs` | Add `quantizer_version` to `ChunkMetadata` + LanceDB schema, wire quantizer into `VectorDB`, add `build_index()` method |
| Modify | `src/main.rs` | Enhance `Reindex` CLI with `--quantizer` and `--store` args |
| Modify | `src/knowledge/articles.rs` | Pass `quantizer_version` when constructing `ChunkMetadata` |
| Modify | `src/search/mod.rs` | Pass quantizer to `VectorDB::open()` |
| Modify | `src/search/indexer.rs` | Pass quantizer to `VectorDB::open()` |
| Modify | `src/connectors/local_files.rs` | Pass `quantizer_version` to `ChunkMetadata` if it constructs one |
| Modify | `Cargo.toml` | Add `turboquant` feature flag |
| Create | `tests/quantizer_integration.rs` | End-to-end: insert, build index, search, switch quantizer, rebuild, search |

---

### Task 1: VectorQuantizer trait and types

**Files:**
- Create: `src/vectordb/quantizer.rs`
- Modify: `src/vectordb/mod.rs` (add `pub mod quantizer;`)

- [ ] **Step 1: Write the failing test**

In `src/vectordb/quantizer.rs`, write a test that asserts the trait is object-safe (can be used as `dyn VectorQuantizer`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Verify the trait is object-safe — this won't compile if it isn't.
    #[allow(dead_code)]
    fn assert_object_safe(_q: &dyn VectorQuantizer) {}
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib vectordb::quantizer -- 2>&1 | head -30`
Expected: FAIL — module and trait don't exist yet.

- [ ] **Step 3: Write the trait, types, and module declaration**

Create `src/vectordb/quantizer.rs`:

```rust
//! Pluggable vector quantizer abstraction over LanceDB index strategies.
//!
//! Each `VectorQuantizer` impl wraps a LanceDB index type (IVF-PQ, IVF-HNSW-SQ)
//! or a custom codec (TurboQuant in P5). The active quantizer is resolved at
//! startup from `knowledge_store.quantizer_version`.

use anyhow::Result;
use arrow_array::RecordBatch;
use async_trait::async_trait;

/// Filter criteria passed to quantizer search.
#[derive(Debug, Clone, Default)]
pub struct QuantizerFilter {
    pub document_id: Option<String>,
    pub source_type: Option<String>,
}

/// Abstraction over LanceDB index strategies.
///
/// Implementations wrap a specific LanceDB `Index` variant or a fully custom
/// codec. `VectorDB` holds an `Arc<dyn VectorQuantizer>` and delegates
/// `build_index` and `search` calls through it.
#[async_trait]
pub trait VectorQuantizer: Send + Sync {
    /// Unique version string stored in chunk metadata for dispatch,
    /// e.g. `"ivf_pq_v1"`, `"int8_v1"`, `"turboquant_v1"`.
    fn version(&self) -> &'static str;

    /// Build or rebuild the vector index on the given LanceDB table.
    ///
    /// For LanceDB-native quantizers this calls `table.create_index()`.
    /// For TurboQuant (P5) this will encode vectors and build a custom index.
    ///
    /// If the table has too few rows for the chosen index strategy, this
    /// should log a warning and return `Ok(())` — brute-force search still
    /// works without an index.
    async fn build_index(&self, table: &lancedb::Table) -> Result<()>;

    /// Execute ANN search, returning raw `RecordBatch`es from LanceDB.
    ///
    /// `VectorDB::search()` handles parsing batches into `VectorSearchResult`.
    async fn search(
        &self,
        table: &lancedb::Table,
        query: &[f32],
        limit: usize,
    ) -> Result<Vec<RecordBatch>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify the trait is object-safe — this won't compile if it isn't.
    #[allow(dead_code)]
    fn assert_object_safe(_q: &dyn VectorQuantizer) {}
}
```

Add the module declaration in `src/vectordb/mod.rs`, after the existing `use` block:

```rust
pub mod quantizer;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib vectordb::quantizer -- 2>&1 | tail -5`
Expected: PASS (test compiles, trait is object-safe)

- [ ] **Step 5: Commit**

```bash
git add src/vectordb/quantizer.rs src/vectordb/mod.rs
git commit -m "feat(vectordb): define VectorQuantizer trait and QuantizerFilter type"
```

---

### Task 2: IvfPqQuantizer implementation

**Files:**
- Modify: `src/vectordb/quantizer.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `quantizer.rs`. This test creates a real LanceDB table, inserts vectors, builds an IVF-PQ index, and searches:

```rust
    use crate::embeddings::EMBEDDING_DIM;
    use arrow_array::{
        builder::{FixedSizeListBuilder, Float32Builder},
        RecordBatch, RecordBatchIterator, StringArray, UInt32Array,
    };
    use arrow_schema::{DataType, Field, Schema};
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Helper: create a minimal chunks table schema for tests.
    fn test_chunks_schema() -> Arc<Schema> {
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
            Field::new("quantizer_version", DataType::Utf8, false),
        ]))
    }

    /// Helper: insert N rows of test data into a chunks table.
    async fn insert_test_chunks(
        table: &lancedb::Table,
        schema: &Arc<Schema>,
        n: usize,
        quantizer_version: &str,
    ) {
        for i in 0..n {
            let mut emb = vec![0.0f32; EMBEDDING_DIM];
            emb[i % EMBEDDING_DIM] = 1.0; // unique direction per row

            let mut builder =
                FixedSizeListBuilder::new(Float32Builder::new(), EMBEDDING_DIM as i32);
            let values = builder.values();
            for &v in &emb {
                values.append_value(v);
            }
            builder.append(true);
            let emb_array = builder.finish();

            let batch = RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(StringArray::from(vec![format!("chunk-{i}")])),
                    Arc::new(StringArray::from(vec![format!("doc-{i}")])),
                    Arc::new(UInt32Array::from(vec![0u32])),
                    Arc::new(UInt32Array::from(vec![1u32])),
                    Arc::new(StringArray::from(vec![format!("/test/{i}.txt")])),
                    Arc::new(StringArray::from(vec![format!("Doc {i}")])),
                    Arc::new(StringArray::from(vec!["local"])),
                    Arc::new(StringArray::from(vec![format!("local:///test/{i}.txt")])),
                    Arc::new(emb_array),
                    Arc::new(StringArray::from(vec!["test text"])),
                    Arc::new(UInt32Array::from(vec![2u32])),
                    Arc::new(UInt32Array::from(vec![9u32])),
                    Arc::new(UInt32Array::from(vec![None::<u32>])),
                    Arc::new(UInt32Array::from(vec![None::<u32>])),
                    Arc::new(UInt32Array::from(vec![0u32])),
                    Arc::new(UInt32Array::from(vec![9u32])),
                    Arc::new(StringArray::from(vec![None::<&str>])),
                    Arc::new(StringArray::from(vec![None::<&str>])),
                    Arc::new(StringArray::from(vec!["2026-01-01T00:00:00Z"])),
                    Arc::new(StringArray::from(vec!["2026-01-01T00:00:00Z"])),
                    Arc::new(StringArray::from(vec![quantizer_version])),
                ],
            )
            .unwrap();

            let batches = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
            table.add(Box::new(batches)).execute().await.unwrap();
        }
    }

    /// Helper: open a temp LanceDB connection and create a chunks table.
    async fn setup_test_table() -> (TempDir, lancedb::Table) {
        let tmp = TempDir::new().unwrap();
        let db = lancedb::connect(tmp.path().to_str().unwrap())
            .execute()
            .await
            .unwrap();

        let schema = test_chunks_schema();
        let batch = RecordBatch::new_empty(schema.clone());
        let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
        let table = db
            .create_table("chunks", Box::new(batches))
            .execute()
            .await
            .unwrap();

        (tmp, table)
    }

    #[tokio::test]
    async fn test_ivf_pq_version() {
        let q = IvfPqQuantizer;
        assert_eq!(q.version(), "ivf_pq_v1");
    }

    #[tokio::test]
    async fn test_ivf_pq_search_without_index() {
        let (_tmp, table) = setup_test_table().await;
        let schema = test_chunks_schema();

        // Insert 10 rows
        insert_test_chunks(&table, &schema, 10, "ivf_pq_v1").await;

        // Search — should work via brute-force even without build_index
        let q = IvfPqQuantizer;
        let mut query = vec![0.0f32; EMBEDDING_DIM];
        query[0] = 1.0;
        let batches = q.search(&table, &query, 5).await.unwrap();
        assert!(!batches.is_empty());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib vectordb::quantizer::tests::test_ivf_pq -- 2>&1 | head -20`
Expected: FAIL — `IvfPqQuantizer` does not exist yet.

- [ ] **Step 3: Implement IvfPqQuantizer**

Add above the `#[cfg(test)]` block in `src/vectordb/quantizer.rs`:

```rust
use futures_util::TryStreamExt;
use lancedb::index::Index;
use lancedb::query::{ExecutableQuery, QueryBase};
use tracing::warn;

/// IVF-PQ quantizer — wraps LanceDB's native IVF with Product Quantization.
///
/// This is the default quantizer for migrated stores. `build_index` creates
/// the index with LanceDB defaults; `search` delegates to `vector_search()`
/// which uses the index if present, otherwise falls back to brute-force.
pub struct IvfPqQuantizer;

#[async_trait]
impl VectorQuantizer for IvfPqQuantizer {
    fn version(&self) -> &'static str {
        "ivf_pq_v1"
    }

    async fn build_index(&self, table: &lancedb::Table) -> Result<()> {
        let row_count = table.count_rows(None).await?;
        if row_count < 256 {
            warn!(
                "IvfPqQuantizer: table has {} rows (< 256), skipping index creation — \
                 brute-force search will be used",
                row_count
            );
            return Ok(());
        }

        table
            .create_index(&["embedding"], Index::IvfPq(Default::default()))
            .execute()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create IVF-PQ index: {}", e))?;

        tracing::info!("IVF-PQ index created on {} rows", row_count);
        Ok(())
    }

    async fn search(
        &self,
        table: &lancedb::Table,
        query: &[f32],
        limit: usize,
    ) -> Result<Vec<RecordBatch>> {
        let stream = table
            .vector_search(query.to_vec())?
            .limit(limit)
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = stream.try_collect().await?;
        Ok(batches)
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib vectordb::quantizer::tests::test_ivf_pq -- 2>&1 | tail -10`
Expected: PASS — both `test_ivf_pq_version` and `test_ivf_pq_search_without_index` pass.

- [ ] **Step 5: Commit**

```bash
git add src/vectordb/quantizer.rs
git commit -m "feat(vectordb): implement IvfPqQuantizer wrapping LanceDB IVF-PQ index"
```

---

### Task 3: Int8Quantizer implementation

**Files:**
- Modify: `src/vectordb/quantizer.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module:

```rust
    #[tokio::test]
    async fn test_int8_version() {
        let q = Int8Quantizer;
        assert_eq!(q.version(), "int8_v1");
    }

    #[tokio::test]
    async fn test_int8_search_without_index() {
        let (_tmp, table) = setup_test_table().await;
        let schema = test_chunks_schema();

        insert_test_chunks(&table, &schema, 10, "int8_v1").await;

        let q = Int8Quantizer;
        let mut query = vec![0.0f32; EMBEDDING_DIM];
        query[0] = 1.0;
        let batches = q.search(&table, &query, 5).await.unwrap();
        assert!(!batches.is_empty());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib vectordb::quantizer::tests::test_int8 -- 2>&1 | head -20`
Expected: FAIL — `Int8Quantizer` does not exist.

- [ ] **Step 3: Implement Int8Quantizer**

Add below `IvfPqQuantizer` in `src/vectordb/quantizer.rs`:

```rust
/// Int8 scalar quantizer — wraps LanceDB's IVF-HNSW with scalar (int8)
/// quantization. Lower memory footprint than IVF-PQ at the cost of slightly
/// reduced recall for high-dimensional vectors.
pub struct Int8Quantizer;

#[async_trait]
impl VectorQuantizer for Int8Quantizer {
    fn version(&self) -> &'static str {
        "int8_v1"
    }

    async fn build_index(&self, table: &lancedb::Table) -> Result<()> {
        let row_count = table.count_rows(None).await?;
        if row_count < 256 {
            warn!(
                "Int8Quantizer: table has {} rows (< 256), skipping index creation — \
                 brute-force search will be used",
                row_count
            );
            return Ok(());
        }

        table
            .create_index(&["embedding"], Index::IvfHnswSq(Default::default()))
            .execute()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create IVF-HNSW-SQ index: {}", e))?;

        tracing::info!("IVF-HNSW-SQ (int8) index created on {} rows", row_count);
        Ok(())
    }

    async fn search(
        &self,
        table: &lancedb::Table,
        query: &[f32],
        limit: usize,
    ) -> Result<Vec<RecordBatch>> {
        let stream = table
            .vector_search(query.to_vec())?
            .limit(limit)
            .execute()
            .await?;

        let batches: Vec<RecordBatch> = stream.try_collect().await?;
        Ok(batches)
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib vectordb::quantizer::tests::test_int8 -- 2>&1 | tail -10`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/vectordb/quantizer.rs
git commit -m "feat(vectordb): implement Int8Quantizer wrapping LanceDB IVF-HNSW-SQ index"
```

---

### Task 4: TurboQuantQuantizer stub (feature-gated)

**Files:**
- Modify: `src/vectordb/quantizer.rs`
- Modify: `Cargo.toml`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module:

```rust
    #[tokio::test]
    async fn test_turboquant_stub_version() {
        let q = TurboQuantQuantizer;
        assert_eq!(q.version(), "turboquant_v1");
    }

    #[tokio::test]
    async fn test_turboquant_stub_build_index_errors() {
        let (_tmp, table) = setup_test_table().await;
        let q = TurboQuantQuantizer;
        let result = q.build_index(&table).await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("not yet available")
        );
    }

    #[tokio::test]
    async fn test_turboquant_stub_search_errors() {
        let (_tmp, table) = setup_test_table().await;
        let q = TurboQuantQuantizer;
        let query = vec![0.0f32; EMBEDDING_DIM];
        let result = q.search(&table, &query, 5).await;
        assert!(result.is_err());
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib vectordb::quantizer::tests::test_turboquant -- 2>&1 | head -20`
Expected: FAIL — `TurboQuantQuantizer` does not exist.

- [ ] **Step 3: Implement TurboQuantQuantizer stub**

Add below `Int8Quantizer` in `src/vectordb/quantizer.rs`:

```rust
/// TurboQuant quantizer — stub for the `turboquant-rs` crate (P5).
///
/// All methods return errors until the crate is integrated. The struct exists
/// so that `QuantizerRegistry` can resolve `"turboquant_v1"` and produce a
/// clear error message if a user selects it prematurely.
pub struct TurboQuantQuantizer;

#[async_trait]
impl VectorQuantizer for TurboQuantQuantizer {
    fn version(&self) -> &'static str {
        "turboquant_v1"
    }

    async fn build_index(&self, _table: &lancedb::Table) -> Result<()> {
        anyhow::bail!(
            "TurboQuant quantizer is not yet available — \
             it will be integrated in P5. Use 'ivf_pq_v1' or 'int8_v1' instead."
        )
    }

    async fn search(
        &self,
        _table: &lancedb::Table,
        _query: &[f32],
        _limit: usize,
    ) -> Result<Vec<RecordBatch>> {
        anyhow::bail!(
            "TurboQuant quantizer is not yet available — \
             it will be integrated in P5. Use 'ivf_pq_v1' or 'int8_v1' instead."
        )
    }
}
```

Add the `turboquant` feature flag to `Cargo.toml` (under `[features]`):

```toml
turboquant = []   # P5: enables TurboQuantQuantizer with the turboquant-rs crate
```

Note: The stub struct is always compiled (no feature gate) so the registry can always resolve the version string and produce a helpful error. The feature gate will wrap the *real* implementation in P5 when the crate dependency is added.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib vectordb::quantizer::tests::test_turboquant -- 2>&1 | tail -10`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/vectordb/quantizer.rs Cargo.toml
git commit -m "feat(vectordb): add TurboQuantQuantizer stub and turboquant feature flag"
```

---

### Task 5: QuantizerRegistry

**Files:**
- Modify: `src/vectordb/quantizer.rs`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module:

```rust
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
        let result = registry.resolve("nonexistent_v99");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown quantizer"));
    }

    #[test]
    fn test_registry_supported_versions() {
        let registry = QuantizerRegistry::new();
        let versions = registry.supported_versions();
        assert!(versions.contains(&"ivf_pq_v1"));
        assert!(versions.contains(&"int8_v1"));
        assert!(versions.contains(&"turboquant_v1"));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib vectordb::quantizer::tests::test_registry -- 2>&1 | head -20`
Expected: FAIL — `QuantizerRegistry` does not exist.

- [ ] **Step 3: Implement QuantizerRegistry**

Add below `TurboQuantQuantizer` in `src/vectordb/quantizer.rs`:

```rust
use std::sync::Arc;

/// Maps `quantizer_version` strings to `VectorQuantizer` implementations.
///
/// Used at startup to resolve the store's `quantizer_version` field, and by
/// the `reindex` CLI to validate the `--quantizer` argument.
pub struct QuantizerRegistry;

impl QuantizerRegistry {
    pub fn new() -> Self {
        Self
    }

    /// Resolve a version string to its quantizer implementation.
    pub fn resolve(&self, version: &str) -> Result<Arc<dyn VectorQuantizer>> {
        match version {
            "ivf_pq_v1" => Ok(Arc::new(IvfPqQuantizer)),
            "int8_v1" => Ok(Arc::new(Int8Quantizer)),
            "turboquant_v1" => Ok(Arc::new(TurboQuantQuantizer)),
            other => anyhow::bail!(
                "Unknown quantizer version: '{}'. Supported: {}",
                other,
                self.supported_versions().join(", ")
            ),
        }
    }

    /// List all supported quantizer version strings.
    pub fn supported_versions(&self) -> Vec<&'static str> {
        vec!["ivf_pq_v1", "int8_v1", "turboquant_v1"]
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib vectordb::quantizer::tests::test_registry -- 2>&1 | tail -10`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/vectordb/quantizer.rs
git commit -m "feat(vectordb): add QuantizerRegistry for version-string dispatch"
```

---

### Task 6: Add `quantizer_version` to ChunkMetadata and LanceDB schema

**Files:**
- Modify: `src/vectordb/mod.rs`

- [ ] **Step 1: Add field to ChunkMetadata struct**

In `src/vectordb/mod.rs`, add a new field to `ChunkMetadata` after the `document_modified_at` field (around line 103):

```rust
    pub document_modified_at: String,

    /// Which VectorQuantizer impl was active when this chunk was indexed.
    pub quantizer_version: String,
}
```

- [ ] **Step 2: Run cargo check to see all broken call sites**

Run: `cargo check 2>&1 | grep "quantizer_version" | head -20`
Expected: errors at every `ChunkMetadata { ... }` construction site.

- [ ] **Step 3: Add column to `chunks_schema()` and `create_chunks_table()`**

In `chunks_schema()` (around line 649), add after the `"document_modified_at"` field:

```rust
            Field::new("document_modified_at", DataType::Utf8, false),
            Field::new("quantizer_version", DataType::Utf8, false),
```

In `create_chunks_table()` (around line 250), the schema is constructed inline — add the same field there after `"document_modified_at"`:

```rust
            Field::new("document_modified_at", DataType::Utf8, false),
            Field::new("quantizer_version", DataType::Utf8, false),
```

- [ ] **Step 4: Add column to `insert_chunk()`**

In `insert_chunk()` (around line 360), add after the `document_modified_at` array:

```rust
                Arc::new(StringArray::from(vec![meta.document_modified_at.as_str()])),
                Arc::new(StringArray::from(vec![meta.quantizer_version.as_str()])),
```

- [ ] **Step 5: Fix all ChunkMetadata construction sites**

There are three places that construct `ChunkMetadata`:

**`src/knowledge/articles.rs`** — in `embed_article()` (around line 99), add after `document_modified_at`:

```rust
                document_modified_at: article.updated_at.clone(),
                quantizer_version: "ivf_pq_v1".to_string(), // TODO(P2-task8): pass from store
```

**`src/vectordb/mod.rs`** — in `insert()` backward-compat method (around line 436), add after `document_modified_at`:

```rust
            document_modified_at: modified_at.to_string(),
            quantizer_version: "ivf_pq_v1".to_string(),
```

**`src/connectors/local_files.rs`** — if it constructs ChunkMetadata, add the same field. Check with `cargo check`.

Any other sites identified by `cargo check` — add `quantizer_version: "ivf_pq_v1".to_string()`.

- [ ] **Step 6: Run cargo check and test**

Run: `cargo check 2>&1 | tail -5`
Expected: no errors

Run: `cargo test --lib vectordb -- 2>&1 | tail -10`
Expected: existing `test_vectordb_operations` passes

- [ ] **Step 7: Commit**

```bash
git add src/vectordb/mod.rs src/knowledge/articles.rs src/connectors/local_files.rs
git commit -m "feat(vectordb): add quantizer_version field to ChunkMetadata and LanceDB schema"
```

---

### Task 7: Wire quantizer into VectorDB

**Files:**
- Modify: `src/vectordb/mod.rs`

- [ ] **Step 1: Write the failing test**

Replace the existing `test_vectordb_operations` test with one that uses a quantizer:

```rust
    #[tokio::test]
    async fn test_vectordb_with_quantizer() {
        use crate::vectordb::quantizer::IvfPqQuantizer;

        let q: Arc<dyn crate::vectordb::quantizer::VectorQuantizer> = Arc::new(IvfPqQuantizer);
        let db = VectorDB::open(q).await.unwrap();

        let embedding = vec![0.1f32; EMBEDDING_DIM];
        db.insert("/test/file.txt", &embedding, 100, "2024-01-01T00:00:00Z")
            .await
            .unwrap();

        let results = db.search(&embedding, 10).await.unwrap();
        assert!(!results.is_empty());

        db.delete("/test/file.txt").await.unwrap();
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib vectordb::tests::test_vectordb_with_quantizer -- 2>&1 | head -20`
Expected: FAIL — `VectorDB::open()` doesn't exist.

- [ ] **Step 3: Add quantizer field and refactor constructor**

In `src/vectordb/mod.rs`, modify `VectorDB`:

```rust
use crate::vectordb::quantizer::VectorQuantizer;

/// Vector database wrapper
pub struct VectorDB {
    db: lancedb::Connection,
    quantizer: Arc<dyn VectorQuantizer>,
}

impl VectorDB {
    /// Create or open the vector database at the default path with a quantizer.
    pub async fn open(quantizer: Arc<dyn VectorQuantizer>) -> Result<Self> {
        let db_path = config::data_dir().join("vectordb");
        std::fs::create_dir_all(&db_path)?;

        info!("Opening vector database at: {}", db_path.display());

        let db = connect(db_path.to_string_lossy().as_ref())
            .execute()
            .await
            .context("Failed to connect to LanceDB")?;

        let vectordb = Self { db, quantizer };
        vectordb.ensure_tables().await?;
        Ok(vectordb)
    }

    /// Backwards-compatible constructor — uses IvfPqQuantizer as default.
    pub async fn new() -> Result<Self> {
        Self::open(Arc::new(quantizer::IvfPqQuantizer)).await
    }
```

- [ ] **Step 4: Wire quantizer into `search()`**

Replace the body of `search()` (around line 444) to delegate to the quantizer:

```rust
    pub async fn search(
        &self,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<VectorSearchResult>> {
        let table = self.db.open_table(CHUNKS_TABLE).execute().await?;
        let batches = self.quantizer.search(&table, query_embedding, limit).await?;

        let mut search_results = Vec::new();
        for batch in batches {
            self.parse_search_batch(&batch, &mut search_results);
        }
        Ok(search_results)
    }
```

Extract the batch-parsing logic into a helper method:

```rust
    /// Parse a RecordBatch from vector search into VectorSearchResult entries.
    fn parse_search_batch(&self, batch: &RecordBatch, results: &mut Vec<VectorSearchResult>) {
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

                results.push(VectorSearchResult {
                    id: chunk_ids.value(i).to_string(),
                    path: paths.value(i).to_string(),
                    title: titles.value(i).to_string(),
                    score,
                    size_bytes: 0,
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
```

- [ ] **Step 5: Add public `build_index()` method**

```rust
    /// Build (or rebuild) the vector index using the active quantizer.
    ///
    /// Call this after bulk inserts or after switching quantizers via `reindex`.
    pub async fn build_index(&self) -> Result<()> {
        let table = self.db.open_table(CHUNKS_TABLE).execute().await?;
        self.quantizer.build_index(&table).await
    }

    /// Return the active quantizer's version string.
    pub fn quantizer_version(&self) -> &str {
        self.quantizer.version()
    }
```

- [ ] **Step 6: Run tests**

Run: `cargo test --lib vectordb -- 2>&1 | tail -10`
Expected: `test_vectordb_with_quantizer` passes

- [ ] **Step 7: Commit**

```bash
git add src/vectordb/mod.rs
git commit -m "feat(vectordb): wire VectorQuantizer into VectorDB search and index paths"
```

---

### Task 8: Update VectorDB consumers

**Files:**
- Modify: `src/main.rs`
- Modify: `src/search/mod.rs`
- Modify: `src/search/indexer.rs`
- Modify: `src/knowledge/articles.rs`
- Modify: `src/k2k/server.rs`
- Modify: `src/k2k/task_handlers.rs`
- Modify: `src/router/mod.rs`
- Modify: `src/router/executor.rs`
- Modify: `src/connectors/local_files.rs`

- [ ] **Step 1: Identify all `VectorDB::new()` call sites**

Run: `cargo check 2>&1 | grep "VectorDB" | head -30`

All existing `VectorDB::new()` calls continue to work (backward-compatible via the `new()` alias). However, call sites that know the store's quantizer should use `VectorDB::open(quantizer)` instead.

- [ ] **Step 2: Update `search_files()` in `src/search/mod.rs`**

The `search_files` function creates a `VectorDB::new()` on line 51. This is fine — `new()` defaults to `IvfPqQuantizer`. No change needed here for P2 since file search doesn't have a store context.

Verify no changes needed:

Run: `cargo check 2>&1 | tail -5`
Expected: compiles clean.

- [ ] **Step 3: Update `ArticleService` in `src/knowledge/articles.rs`**

`ArticleService` receives `Arc<VectorDB>` in its constructor — it doesn't create the VectorDB. The quantizer is already wired at VectorDB creation time. No change needed in `articles.rs` beyond what was done in Task 6 (adding `quantizer_version` to `ChunkMetadata`).

However, the hardcoded `"ivf_pq_v1"` in `embed_article()` should use the VectorDB's quantizer version. Update the `embed_article` method — change the `ChunkMetadata` construction:

```rust
                quantizer_version: self.vectordb.quantizer_version().to_string(),
```

This requires `VectorDB::quantizer_version()` which was added in Task 7.

- [ ] **Step 4: Update `src/main.rs` startup**

In `cmd_start()` (or wherever VectorDB is created for the server), the VectorDB should be created with the store's quantizer version. Find where `VectorDB::new()` is called during server startup and update it:

```rust
use crate::vectordb::quantizer::QuantizerRegistry;

// In the startup function, after loading the store:
let registry = QuantizerRegistry::new();
let quantizer = registry.resolve(&store.quantizer_version)?;
let vectordb = Arc::new(VectorDB::open(quantizer).await?);
```

If VectorDB is created before a store is available, keep using `VectorDB::new()` (defaults to IvfPq).

- [ ] **Step 5: Run full check and tests**

Run: `cargo check 2>&1 | tail -5`
Expected: compiles clean

Run: `cargo test 2>&1 | tail -20`
Expected: all existing tests pass

- [ ] **Step 6: Commit**

```bash
git add src/knowledge/articles.rs src/main.rs src/search/mod.rs
git commit -m "feat(vectordb): update consumers to use quantizer-aware VectorDB"
```

---

### Task 9: Enhance Reindex CLI with `--quantizer` and `--store`

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Add CLI args to Reindex command**

In `src/main.rs`, modify the `Reindex` variant in the `Commands` enum:

```rust
    /// Reindex all files
    Reindex {
        /// Force full reindex (ignore cache)
        #[arg(long)]
        force: bool,

        /// Rebuild LanceDB index with a specific quantizer.
        /// Supported: ivf_pq_v1, int8_v1, turboquant_v1
        #[arg(long)]
        quantizer: Option<String>,

        /// Target store ID (required when --quantizer is set)
        #[arg(long)]
        store: Option<String>,
    },
```

- [ ] **Step 2: Update the match arm**

Update the `Commands::Reindex` match arm:

```rust
            Commands::Reindex { force, quantizer, store } => {
                if let Some(ref qv) = quantizer {
                    let store_id = store.as_deref().ok_or_else(|| {
                        anyhow::anyhow!("--store is required when --quantizer is specified")
                    })?;
                    cmd_reindex_quantizer(qv, store_id).await?;
                } else {
                    cmd_reindex(force).await?;
                }
            }
```

- [ ] **Step 3: Implement `cmd_reindex_quantizer()`**

Add a new function in `src/main.rs`:

```rust
async fn cmd_reindex_quantizer(quantizer_version: &str, store_id: &str) -> Result<()> {
    use crate::vectordb::quantizer::QuantizerRegistry;

    let registry = QuantizerRegistry::new();

    // Validate the quantizer version before doing any work
    let quantizer = registry.resolve(quantizer_version)?;

    info!(
        "Rebuilding LanceDB index for store '{}' with quantizer '{}'",
        store_id, quantizer_version
    );

    // Open SurrealDB to verify store exists and update quantizer_version
    let cfg = config::load_config().await?;
    let db = crate::store::SurrealStore::open(&cfg).await?;

    let store = db
        .get_knowledge_store(store_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Store '{}' not found", store_id))?;

    println!(
        "Store '{}': switching quantizer from '{}' to '{}'",
        store.name, store.quantizer_version, quantizer_version
    );

    // Open VectorDB with the new quantizer and rebuild the index
    let vectordb = VectorDB::open(quantizer).await?;
    vectordb.build_index().await?;

    // Update the store's quantizer_version in SurrealDB
    let mut updated_store = store;
    updated_store.quantizer_version = quantizer_version.to_string();
    updated_store.updated_at = chrono::Utc::now().to_rfc3339();
    db.update_knowledge_store(&updated_store).await?;

    println!(
        "Reindex complete. Store now uses quantizer '{}'.",
        quantizer_version
    );
    Ok(())
}
```

- [ ] **Step 4: Run cargo check**

Run: `cargo check 2>&1 | tail -10`
Expected: compiles clean. If `update_knowledge_store` doesn't exist on the Store trait, check for the equivalent method name (it may be called something else — check the Store trait in `src/store/mod.rs`).

- [ ] **Step 5: Test CLI arg parsing**

Run: `cargo run -- reindex --help 2>&1`
Expected: shows `--quantizer` and `--store` options

Run: `cargo run -- reindex --quantizer ivf_pq_v1 2>&1`
Expected: error about `--store` being required

- [ ] **Step 6: Commit**

```bash
git add src/main.rs
git commit -m "feat(cli): add --quantizer and --store args to reindex command"
```

---

### Task 10: Integration test — quantizer round-trip

**Files:**
- Create: `tests/quantizer_integration.rs`

- [ ] **Step 1: Write the integration test**

Create `tests/quantizer_integration.rs`:

```rust
//! Integration test: insert documents, build index with different quantizers, search.

use std::sync::Arc;

use arrow_array::{
    builder::{FixedSizeListBuilder, Float32Builder},
    RecordBatch, RecordBatchIterator, StringArray, UInt32Array,
};
use arrow_schema::{DataType, Field, Schema};
use futures_util::TryStreamExt;
use lancedb::connect;
use tempfile::TempDir;

/// Embedding dimension must match the crate's constant
const EMBEDDING_DIM: usize = 384;

fn test_chunks_schema() -> Arc<Schema> {
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
        Field::new("quantizer_version", DataType::Utf8, false),
    ]))
}

fn make_embedding(dimension_idx: usize) -> Vec<f32> {
    let mut emb = vec![0.0f32; EMBEDDING_DIM];
    emb[dimension_idx % EMBEDDING_DIM] = 1.0;
    emb
}

async fn insert_row(
    table: &lancedb::Table,
    schema: &Arc<Schema>,
    idx: usize,
    quantizer_version: &str,
) {
    let emb = make_embedding(idx);
    let mut builder = FixedSizeListBuilder::new(Float32Builder::new(), EMBEDDING_DIM as i32);
    let values = builder.values();
    for &v in &emb {
        values.append_value(v);
    }
    builder.append(true);
    let emb_array = builder.finish();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(vec![format!("chunk-{idx}")])),
            Arc::new(StringArray::from(vec![format!("doc-{idx}")])),
            Arc::new(UInt32Array::from(vec![0u32])),
            Arc::new(UInt32Array::from(vec![1u32])),
            Arc::new(StringArray::from(vec![format!("/test/{idx}.txt")])),
            Arc::new(StringArray::from(vec![format!("Doc {idx}")])),
            Arc::new(StringArray::from(vec!["local"])),
            Arc::new(StringArray::from(vec![format!("local:///test/{idx}.txt")])),
            Arc::new(emb_array),
            Arc::new(StringArray::from(vec!["test content"])),
            Arc::new(UInt32Array::from(vec![2u32])),
            Arc::new(UInt32Array::from(vec![12u32])),
            Arc::new(UInt32Array::from(vec![None::<u32>])),
            Arc::new(UInt32Array::from(vec![None::<u32>])),
            Arc::new(UInt32Array::from(vec![0u32])),
            Arc::new(UInt32Array::from(vec![12u32])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(StringArray::from(vec![None::<&str>])),
            Arc::new(StringArray::from(vec!["2026-01-01T00:00:00Z"])),
            Arc::new(StringArray::from(vec!["2026-01-01T00:00:00Z"])),
            Arc::new(StringArray::from(vec![quantizer_version])),
        ],
    )
    .unwrap();

    let batches = RecordBatchIterator::new(vec![Ok(batch)], schema.clone());
    table.add(Box::new(batches)).execute().await.unwrap();
}

#[tokio::test]
async fn test_quantizer_switch_ivf_pq_to_int8() {
    use knowledge_nexus_agent::vectordb::quantizer::{
        IvfPqQuantizer, Int8Quantizer, QuantizerRegistry, VectorQuantizer,
    };

    let tmp = TempDir::new().unwrap();
    let db = connect(tmp.path().to_str().unwrap())
        .execute()
        .await
        .unwrap();

    let schema = test_chunks_schema();
    let empty = RecordBatch::new_empty(schema.clone());
    let batches = RecordBatchIterator::new(vec![Ok(empty)], schema.clone());
    let table = db
        .create_table("chunks", Box::new(batches))
        .execute()
        .await
        .unwrap();

    // Insert 300 rows (enough for index creation)
    for i in 0..300 {
        insert_row(&table, &schema, i, "ivf_pq_v1").await;
    }

    // Build IVF-PQ index
    let ivf_pq = IvfPqQuantizer;
    ivf_pq.build_index(&table).await.unwrap();

    // Search with IVF-PQ
    let query = make_embedding(0);
    let results = ivf_pq.search(&table, &query, 5).await.unwrap();
    assert!(!results.is_empty(), "IVF-PQ search should return results");

    // Switch to Int8
    let int8 = Int8Quantizer;
    int8.build_index(&table).await.unwrap();

    // Search with Int8
    let results = int8.search(&table, &query, 5).await.unwrap();
    assert!(!results.is_empty(), "Int8 search should return results");

    // Registry resolves both
    let registry = QuantizerRegistry::new();
    let q = registry.resolve("ivf_pq_v1").unwrap();
    assert_eq!(q.version(), "ivf_pq_v1");
    let q = registry.resolve("int8_v1").unwrap();
    assert_eq!(q.version(), "int8_v1");
}

#[tokio::test]
async fn test_turboquant_stub_rejects_operations() {
    use knowledge_nexus_agent::vectordb::quantizer::{TurboQuantQuantizer, VectorQuantizer};

    let tmp = TempDir::new().unwrap();
    let db = connect(tmp.path().to_str().unwrap())
        .execute()
        .await
        .unwrap();

    let schema = test_chunks_schema();
    let empty = RecordBatch::new_empty(schema.clone());
    let batches = RecordBatchIterator::new(vec![Ok(empty)], schema);
    let table = db
        .create_table("chunks", Box::new(batches))
        .execute()
        .await
        .unwrap();

    let tq = TurboQuantQuantizer;
    assert_eq!(tq.version(), "turboquant_v1");

    let err = tq.build_index(&table).await.unwrap_err();
    assert!(
        err.to_string().contains("not yet available"),
        "Expected 'not yet available' error, got: {}",
        err
    );

    let query = vec![0.0f32; EMBEDDING_DIM];
    let err = tq.search(&table, &query, 5).await.unwrap_err();
    assert!(err.to_string().contains("not yet available"));
}

#[tokio::test]
async fn test_search_returns_nearest_neighbor() {
    use knowledge_nexus_agent::vectordb::quantizer::{IvfPqQuantizer, VectorQuantizer};

    let tmp = TempDir::new().unwrap();
    let db = connect(tmp.path().to_str().unwrap())
        .execute()
        .await
        .unwrap();

    let schema = test_chunks_schema();
    let empty = RecordBatch::new_empty(schema.clone());
    let batches = RecordBatchIterator::new(vec![Ok(empty)], schema.clone());
    let table = db
        .create_table("chunks", Box::new(batches))
        .execute()
        .await
        .unwrap();

    // Insert 5 rows pointing in different directions
    for i in 0..5 {
        insert_row(&table, &schema, i, "ivf_pq_v1").await;
    }

    // Query for dimension 2 — chunk-2 should be the best match
    let query = make_embedding(2);
    let ivf_pq = IvfPqQuantizer;
    let batches = ivf_pq.search(&table, &query, 1).await.unwrap();

    assert!(!batches.is_empty());
    let batch = &batches[0];
    let chunk_ids = batch
        .column_by_name("chunk_id")
        .unwrap()
        .as_any()
        .downcast_ref::<StringArray>()
        .unwrap();
    assert_eq!(chunk_ids.value(0), "chunk-2");
}
```

- [ ] **Step 2: Run integration test**

Run: `cargo test --test quantizer_integration -- 2>&1 | tail -20`
Expected: All three tests pass.

If any test fails, diagnose and fix. Common issues:
- Import paths may need adjustment based on the crate's `pub` visibility
- LanceDB index creation may need more rows (the 256 minimum threshold)

- [ ] **Step 3: Commit**

```bash
git add tests/quantizer_integration.rs
git commit -m "test: add quantizer integration tests for IVF-PQ, Int8, and TurboQuant stub"
```

---

### Task 11: Ensure public API visibility

**Files:**
- Modify: `src/vectordb/mod.rs`
- Modify: `src/lib.rs` (or `src/main.rs` if this is a binary-only crate)

- [ ] **Step 1: Check crate structure**

Run: `head -5 src/lib.rs 2>/dev/null || echo "no lib.rs — binary crate"`

If binary-only: integration tests can't import `knowledge_nexus_agent::vectordb::quantizer`. Either:
- Create a minimal `src/lib.rs` that re-exports modules, or
- Move integration tests to `src/vectordb/quantizer.rs` as `#[cfg(test)]` tests

If `lib.rs` exists: ensure `pub mod vectordb;` is declared and `vectordb::quantizer` is `pub`.

- [ ] **Step 2: Ensure `pub mod quantizer` in vectordb/mod.rs**

Already done in Task 1. Verify it's there.

- [ ] **Step 3: Ensure all types needed by integration tests are `pub`**

The following must be `pub`:
- `VectorQuantizer` trait
- `IvfPqQuantizer` struct
- `Int8Quantizer` struct
- `TurboQuantQuantizer` struct
- `QuantizerRegistry` struct + `new()`, `resolve()`, `supported_versions()`
- `QuantizerFilter` struct

All were written as `pub` in earlier tasks. Verify with `cargo test --test quantizer_integration 2>&1 | head -20`.

- [ ] **Step 4: Fix any visibility issues and re-run**

Run: `cargo test --test quantizer_integration 2>&1 | tail -10`
Expected: PASS

- [ ] **Step 5: Run full test suite**

Run: `cargo test 2>&1 | tail -20`
Expected: all tests pass

- [ ] **Step 6: Commit (if changes were needed)**

```bash
git add src/lib.rs src/vectordb/mod.rs
git commit -m "chore: ensure quantizer types are publicly visible for integration tests"
```

---

## Summary of deliverables

| Artifact | Description |
|----------|-------------|
| `src/vectordb/quantizer.rs` | `VectorQuantizer` trait, `QuantizerFilter`, `QuantizerRegistry`, `IvfPqQuantizer`, `Int8Quantizer`, `TurboQuantQuantizer` stub |
| `src/vectordb/mod.rs` | `quantizer_version` field on `ChunkMetadata`, `VectorDB::open(quantizer)`, `build_index()`, `quantizer_version()`, `parse_search_batch()` helper |
| `src/main.rs` | `Reindex --quantizer --store` CLI args, `cmd_reindex_quantizer()` |
| `Cargo.toml` | `turboquant` feature flag |
| `tests/quantizer_integration.rs` | End-to-end tests for IVF-PQ, Int8, TurboQuant stub, nearest-neighbor correctness |

## What this does NOT include (deferred)

- **TurboQuant real implementation** — P5, depends on `turboquant-rs` crate
- **Entity extraction and graph** — P3
- **Tri-signal hybrid retrieval** — P4
- **LanceDB schema migration for existing data** — existing chunks tables don't have `quantizer_version`; a migration helper to add the column with default `"ivf_pq_v1"` may be needed if users have existing data. This is a one-line ALTER equivalent in LanceDB (add column with default). Defer to a follow-up if needed.
