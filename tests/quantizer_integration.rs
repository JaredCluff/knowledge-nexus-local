//! End-to-end integration tests for the quantizer round-trip.
//!
//! Covers:
//!   - IVF-PQ index build + search with 300 rows
//!   - Switching to Int8 quantizer, rebuilding, and searching again
//!   - TurboQuant stub rejects all operations
//!   - Nearest-neighbor search returns the correct top result

use arrow_array::{
    builder::{FixedSizeListBuilder, Float32Builder},
    RecordBatch, RecordBatchIterator, StringArray, UInt32Array,
};
use arrow_schema::{DataType, Field, Schema};
use knowledge_nexus_agent::embeddings::EMBEDDING_DIM;
use knowledge_nexus_agent::vectordb::quantizer::{
    Int8Quantizer, IvfPqQuantizer, QuantizerFilter, QuantizerRegistry, TurboQuantQuantizer,
    VectorQuantizer,
};
use lancedb::connect;
use std::sync::Arc;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Full chunks schema matching the production definition in vectordb/mod.rs.
fn chunks_schema() -> Arc<Schema> {
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

/// Build a 384-d embedding with 1.0 at `dim_idx` and 0.0 everywhere else.
fn make_embedding(dim_idx: usize) -> Vec<f32> {
    let mut e = vec![0.0f32; EMBEDDING_DIM];
    e[dim_idx % EMBEDDING_DIM] = 1.0;
    e
}

/// Build a FixedSizeList<Float32> arrow array from a flat slice of row embeddings.
fn build_embedding_column(embeddings: &[Vec<f32>]) -> Arc<dyn arrow_array::Array> {
    let mut builder = FixedSizeListBuilder::new(Float32Builder::new(), EMBEDDING_DIM as i32);
    for emb in embeddings {
        let vals = builder.values();
        for &v in emb {
            vals.append_value(v);
        }
        builder.append(true);
    }
    Arc::new(builder.finish())
}

/// Create a LanceDB `chunks` table in `dir` and insert `n_rows` rows.
///
/// Embeddings are varied by row so that the SQ quantizer always has a
/// non-zero range (avoids the degenerate min==max case in lance-index).
/// Row i has all values set to `(i as f32 + 1.0) / n_rows as f32`, giving a
/// range of distinct vectors spread across (0, 1].
async fn create_varied_table(dir: &TempDir, n_rows: usize) -> lancedb::Table {
    let schema = chunks_schema();
    let db = connect(dir.path().to_string_lossy().as_ref())
        .execute()
        .await
        .unwrap();

    let embeddings: Vec<Vec<f32>> = (0..n_rows)
        .map(|i| vec![(i as f32 + 1.0) / n_rows as f32; EMBEDDING_DIM])
        .collect();
    insert_rows_into_db(&db, schema, n_rows, &embeddings).await
}

/// Insert a batch of rows into a newly-created `chunks` table.
///
/// `embeddings` must have exactly `n_rows` entries.
async fn insert_rows_into_db(
    db: &lancedb::Connection,
    schema: Arc<Schema>,
    n_rows: usize,
    embeddings: &[Vec<f32>],
) -> lancedb::Table {
    assert_eq!(embeddings.len(), n_rows);

    let chunk_ids: Vec<String> = (0..n_rows).map(|i| format!("chunk-{}", i)).collect();
    let doc_ids: Vec<String> = (0..n_rows).map(|i| format!("doc-{}", i)).collect();
    let paths: Vec<String> = (0..n_rows)
        .map(|i| format!("/path/file{}.txt", i))
        .collect();
    let titles: Vec<String> = (0..n_rows).map(|i| format!("Title {}", i)).collect();
    let texts: Vec<String> = (0..n_rows).map(|i| format!("text content {}", i)).collect();
    let source_uris: Vec<String> = (0..n_rows)
        .map(|i| format!("local:///path/file{}.txt", i))
        .collect();
    let date = "2024-01-01T00:00:00Z";

    let embedding_col = build_embedding_column(embeddings);

    let chunk_id_arr: Vec<&str> = chunk_ids.iter().map(|s| s.as_str()).collect();
    let doc_id_arr: Vec<&str> = doc_ids.iter().map(|s| s.as_str()).collect();
    let path_arr: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
    let title_arr: Vec<&str> = titles.iter().map(|s| s.as_str()).collect();
    let text_arr: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
    let source_uri_arr: Vec<&str> = source_uris.iter().map(|s| s.as_str()).collect();
    let source_type_arr: Vec<&str> = (0..n_rows).map(|_| "local").collect();
    let date_arr: Vec<&str> = (0..n_rows).map(|_| date).collect();
    let chunk_indices: Vec<u32> = (0..n_rows as u32).collect();
    let totals: Vec<u32> = vec![n_rows as u32; n_rows];
    let zeros_u32: Vec<u32> = vec![0u32; n_rows];
    let null_str_arr: Vec<Option<&str>> = vec![None; n_rows];
    let null_u32_arr: Vec<Option<u32>> = vec![None; n_rows];

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(StringArray::from(chunk_id_arr)),
            Arc::new(StringArray::from(doc_id_arr)),
            Arc::new(UInt32Array::from(chunk_indices)),
            Arc::new(UInt32Array::from(totals)),
            Arc::new(StringArray::from(path_arr)),
            Arc::new(StringArray::from(title_arr)),
            Arc::new(StringArray::from(source_type_arr)),
            Arc::new(StringArray::from(source_uri_arr)),
            embedding_col,
            Arc::new(StringArray::from(text_arr)),
            Arc::new(UInt32Array::from(zeros_u32.clone())), // token_count
            Arc::new(UInt32Array::from(zeros_u32.clone())), // char_count
            Arc::new(UInt32Array::from(null_u32_arr.clone())), // start_line (nullable)
            Arc::new(UInt32Array::from(null_u32_arr.clone())), // end_line (nullable)
            Arc::new(UInt32Array::from(zeros_u32.clone())), // start_char
            Arc::new(UInt32Array::from(zeros_u32.clone())), // end_char
            Arc::new(StringArray::from(null_str_arr.clone())), // heading (nullable)
            Arc::new(StringArray::from(null_str_arr.clone())), // parent_heading (nullable)
            Arc::new(StringArray::from(date_arr.clone())), // indexed_at
            Arc::new(StringArray::from(date_arr.clone())), // document_modified_at
            Arc::new(StringArray::from(
                (0..n_rows).map(|_| "ivf_pq_v1").collect::<Vec<_>>(),
            )), // quantizer_version
        ],
    )
    .unwrap();

    let batches = RecordBatchIterator::new(vec![Ok(batch)], schema);
    db.create_table("chunks", Box::new(batches))
        .execute()
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Verify that IVF-PQ index build + search succeeds, then switching to Int8
/// rebuilds and searches successfully, and that QuantizerRegistry resolves both.
#[tokio::test]
async fn test_quantizer_switch_ivf_pq_to_int8() {
    let dir = TempDir::new().unwrap();
    let table = create_varied_table(&dir, 300).await;

    // --- IVF-PQ phase ---
    let ivf_pq = IvfPqQuantizer;
    ivf_pq
        .build_index(&table)
        .await
        .expect("IVF-PQ build_index should succeed with 300 rows");

    let query = vec![0.1f32; EMBEDDING_DIM];
    let filter = QuantizerFilter {
        limit: Some(5),
        ..Default::default()
    };
    let results = ivf_pq
        .search(&table, &query, &filter)
        .await
        .expect("IVF-PQ search should succeed");
    let total: usize = results.iter().map(|b| b.num_rows()).sum();
    assert!(
        total > 0,
        "IVF-PQ search returned no rows; expected at least one result"
    );

    // --- Int8 phase (replaces IVF-PQ index on the same table) ---
    let int8 = Int8Quantizer;
    int8.build_index(&table)
        .await
        .expect("Int8 build_index should succeed with 300 rows");

    let results2 = int8
        .search(&table, &query, &filter)
        .await
        .expect("Int8 search should succeed after rebuilding index");
    let total2: usize = results2.iter().map(|b| b.num_rows()).sum();
    assert!(
        total2 > 0,
        "Int8 search returned no rows; expected at least one result"
    );

    // --- QuantizerRegistry resolves both versions ---
    let registry = QuantizerRegistry::new();

    let q_ivf = registry.resolve("ivf_pq_v1").unwrap();
    assert_eq!(q_ivf.version(), "ivf_pq_v1");

    let q_int8 = registry.resolve("int8_v1").unwrap();
    assert_eq!(q_int8.version(), "int8_v1");
}

/// Verify that TurboQuantQuantizer.version() is correct and that both
/// build_index() and search() return errors containing "not yet available".
#[tokio::test]
async fn test_turboquant_stub_rejects_operations() {
    let dir = TempDir::new().unwrap();
    let table = create_varied_table(&dir, 10).await;

    let q = TurboQuantQuantizer;

    assert_eq!(q.version(), "turboquant_v1");

    let build_err = q
        .build_index(&table)
        .await
        .expect_err("TurboQuantQuantizer.build_index() should return an error");
    assert!(
        build_err.to_string().contains("not yet available"),
        "build_index error should mention 'not yet available', got: {}",
        build_err
    );

    let query = vec![0.0f32; EMBEDDING_DIM];
    let filter = QuantizerFilter::default();
    let search_err = q
        .search(&table, &query, &filter)
        .await
        .expect_err("TurboQuantQuantizer.search() should return an error");
    assert!(
        search_err.to_string().contains("not yet available"),
        "search error should mention 'not yet available', got: {}",
        search_err
    );
}

/// Verify that the nearest-neighbor search returns the correct top result.
///
/// Inserts 5 rows whose embeddings point along orthogonal dimensions
/// (row i has embedding[i] = 1.0, all others 0.0), then searches for
/// dimension 2 and asserts that "chunk-2" is the top hit.
#[tokio::test]
async fn test_search_returns_nearest_neighbor() {
    let dir = TempDir::new().unwrap();
    let schema = chunks_schema();
    let db = connect(dir.path().to_string_lossy().as_ref())
        .execute()
        .await
        .unwrap();

    let n_rows: usize = 5;
    let embeddings: Vec<Vec<f32>> = (0..n_rows).map(|i| make_embedding(i)).collect();
    let table = insert_rows_into_db(&db, schema, n_rows, &embeddings).await;

    // Search using the dimension-2 unit vector — nearest neighbour is chunk-2.
    let q = IvfPqQuantizer;
    // build_index is a no-op here (< 256 rows), which is fine; LanceDB falls
    // back to a brute-force scan.
    q.build_index(&table)
        .await
        .expect("build_index with < 256 rows should silently skip");

    let mut query = vec![0.0f32; EMBEDDING_DIM];
    query[2] = 1.0;

    let filter = QuantizerFilter {
        limit: Some(1),
        ..Default::default()
    };
    let results = q
        .search(&table, &query, &filter)
        .await
        .expect("search should succeed");

    assert!(
        !results.is_empty(),
        "search returned no RecordBatches at all"
    );

    let first_batch = &results[0];
    assert!(
        first_batch.num_rows() > 0,
        "first RecordBatch has no rows"
    );

    let chunk_id_col = first_batch
        .column_by_name("chunk_id")
        .expect("RecordBatch should have a 'chunk_id' column")
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("chunk_id column should be StringArray");

    let top_id = chunk_id_col.value(0);
    assert_eq!(
        top_id, "chunk-2",
        "nearest-neighbor search for dimension 2 should return 'chunk-2', got '{}'",
        top_id
    );
}
