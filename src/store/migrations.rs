//! Schema version tracking and data migrations for SurrealDB.
//!
//! On every `SurrealStore::open`, we run `schema::ddl()` (which is idempotent),
//! then record the version in the `_schema_version` table.
//!
//! P3 adds a data migration: tags are converted from the article JSON array
//! into `tag` records with `TAGGED` edges.

use anyhow::{Context, Result};
use surrealdb::engine::any::Any;
use surrealdb::Surreal;

use super::schema;
use super::slugify::slugify;

/// Row shape for reading articles during tag migration.
#[derive(serde::Deserialize)]
struct ArticleTagRow {
    id: String,
    store_id: String,
    tags: serde_json::Value,
}

pub async fn run_migrations(db: &Surreal<Any>) -> Result<()> {
    // Apply DDL (idempotent — IF NOT EXISTS / OVERWRITE)
    db.query(schema::ddl())
        .await
        .context("Failed to apply SurrealDB DDL")?
        .check()
        .context("SurrealDB DDL returned an error")?;

    // Check current schema version
    let mut resp = db
        .query("SELECT version FROM _schema_version WHERE id = type::thing('_schema_version', 'current')")
        .await
        .context("Failed to read schema version")?;
    let versions: Vec<serde_json::Value> = resp.take(0).unwrap_or_default();
    let current_version = versions
        .first()
        .and_then(|v| v.get("version"))
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0");

    // Run P3 data migration if upgrading from P1 or P2
    if current_version.starts_with("1.0.0-p1") || current_version.starts_with("1.0.0-p2") {
        tracing::info!("Running P3 tag migration from version {}", current_version);
        migrate_tags_to_edges(db).await?;
    }

    // Run P5 multi-graph migration if upgrading from P3 (or earlier)
    if current_version.starts_with("1.0.0-p1")
        || current_version.starts_with("1.0.0-p2")
        || current_version.starts_with("1.0.0-p3")
    {
        tracing::info!("Running P5 multi-graph migration from version {}", current_version);
        migrate_related_to_to_entity_overlap(db).await?;
    }

    // Record current schema version
    let applied_at = chrono::Utc::now().to_rfc3339();
    db.query(
        "UPSERT type::thing('_schema_version', 'current') CONTENT { version: $version, applied_at: $applied_at }",
    )
    .bind(("version", schema::SCHEMA_VERSION))
    .bind(("applied_at", applied_at))
    .await
    .context("Failed to record schema version")?
    .check()
    .context("Schema version write returned an error")?;

    tracing::info!("SurrealDB schema at version {}", schema::SCHEMA_VERSION);
    Ok(())
}

/// Migrate article tags from the JSON array field to `tag` records + `TAGGED` edges.
///
/// For each article with a non-empty `tags` array:
/// 1. For each tag string: upsert a `tag` record (slugified ID, scoped to store_id).
/// 2. Create a `TAGGED` edge from the article to the tag.
/// 3. After all articles are processed, remove the `tags` field from the article schema.
async fn migrate_tags_to_edges(db: &Surreal<Any>) -> Result<()> {
    // Read all articles with their tags
    let mut resp = db
        .query("SELECT meta::id(id) AS id, store_id, tags FROM article")
        .await
        .context("Failed to read articles for tag migration")?;
    let articles: Vec<ArticleTagRow> = resp.take(0).unwrap_or_default();

    let mut tag_count = 0u64;
    let mut edge_count = 0u64;
    let now = chrono::Utc::now().to_rfc3339();

    for article in &articles {
        let tags = match &article.tags {
            serde_json::Value::Array(arr) => arr.clone(),
            _ => continue,
        };

        for tag_val in &tags {
            let tag_name = match tag_val.as_str() {
                Some(s) if !s.trim().is_empty() => s.trim().to_string(),
                _ => continue,
            };

            let tag_id = slugify(&tag_name);

            // Upsert tag record
            db.query(
                "UPSERT type::thing('tag', $id) CONTENT {
                    name: $name, store_id: $store_id,
                    created_at: $created_at
                }",
            )
            .bind(("id", tag_id.clone()))
            .bind(("name", tag_name))
            .bind(("store_id", article.store_id.clone()))
            .bind(("created_at", now.clone()))
            .await
            .context("Failed to upsert tag during migration")?
            .check()?;
            tag_count += 1;

            // Create TAGGED edge (ignore errors from duplicate edges)
            let edge_result = db
                .query(
                    "LET $from = type::thing('article', $article_id);
                     LET $to = type::thing('tag', $tag_id);
                     RELATE $from->tagged->$to CONTENT {
                        created_at: $created_at
                    }",
                )
                .bind(("article_id", article.id.clone()))
                .bind(("tag_id", tag_id))
                .bind(("created_at", now.clone()))
                .await;

            match edge_result {
                Ok(r) => { let _ = r.check(); }
                Err(e) => {
                    tracing::warn!("Skipping duplicate TAGGED edge for article {}: {}", article.id, e);
                }
            }
            edge_count += 1;
        }
    }

    // NOTE: We intentionally do NOT remove the `tags` field from the article
    // schema. The DDL in schema.rs defines it (for backward compat), and
    // removing it here would be undone on the next startup when DDL re-runs.
    // The field stays as dead weight; canonical tag data lives in `tag` records
    // + `TAGGED` edges.

    tracing::info!(
        "P3 tag migration complete: {} tags upserted, {} TAGGED edges created",
        tag_count, edge_count
    );
    Ok(())
}

/// P5 migration: copy `related_to` edges into the new `entity_overlap` table,
/// preserving Jaccard-derived `shared_entity_count` and `strength`, defaulting
/// `confidence = strength`, `extraction_method = "heuristic"`. Then deletes
/// all rows from `related_to`. The old table itself is kept in DDL for
/// backward compatibility but holds no data going forward.
async fn migrate_related_to_to_entity_overlap(db: &Surreal<Any>) -> Result<()> {
    // Read all related_to edges
    let mut resp = db
        .query(
            "SELECT meta::id(in) AS from_id, meta::id(out) AS to_id,
                    store_id, shared_entity_count, strength,
                    created_at, updated_at
             FROM related_to"
        )
        .await
        .context("Failed to read related_to edges during P5 migration")?;

    #[derive(serde::Deserialize)]
    struct OldEdge {
        from_id: String,
        to_id: String,
        store_id: Option<String>,
        shared_entity_count: i64,
        strength: f64,
        created_at: String,
        updated_at: String,
    }

    let edges: Vec<OldEdge> = resp.take(0).unwrap_or_default();
    let total = edges.len();
    let mut migrated = 0u64;

    for e in edges {
        let store_id = e.store_id.unwrap_or_default();
        let res = db
            .query(
                "LET $from = type::thing('article', $from_id);
                 LET $to = type::thing('article', $to_id);
                 RELATE $from->entity_overlap->$to CONTENT {
                    shared_entity_count: $cnt,
                    strength: $strength,
                    confidence: $confidence,
                    extraction_method: 'heuristic',
                    store_id: $store_id,
                    created_at: $created_at,
                    updated_at: $updated_at
                 }"
            )
            .bind(("from_id", e.from_id.clone()))
            .bind(("to_id", e.to_id.clone()))
            .bind(("cnt", e.shared_entity_count))
            .bind(("strength", e.strength))
            .bind(("confidence", e.strength))
            .bind(("store_id", store_id))
            .bind(("created_at", e.created_at))
            .bind(("updated_at", e.updated_at))
            .await;

        match res {
            Ok(r) => { let _ = r.check(); migrated += 1; }
            Err(err) => {
                tracing::warn!(
                    "Skipping duplicate entity_overlap edge {} -> {} during P5 migration: {}",
                    e.from_id, e.to_id, err
                );
            }
        }
    }

    // Delete all rows from the old related_to table
    db.query("DELETE related_to")
        .await
        .context("Failed to drop related_to rows after P5 migration")?
        .check()
        .context("DELETE related_to returned an error")?;

    tracing::info!(
        "P5 migration complete: {}/{} related_to edges renamed to entity_overlap",
        migrated, total
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::schema;
    use surrealdb::engine::any::connect;

    /// Helper: connect to an in-memory SurrealDB and seed some `related_to`
    /// edges that simulate a P3-state corpus.
    async fn setup_p3_corpus() -> Surreal<Any> {
        let db = connect("memory").await.expect("connect mem");
        db.use_ns("test").use_db("test").await.expect("use ns/db");
        db.query(schema::ddl()).await.expect("ddl").check().expect("ddl check");

        // Pretend schema version is P3
        db.query(
            "UPSERT type::thing('_schema_version', 'current') CONTENT { version: $v, applied_at: $t }"
        )
        .bind(("v", "1.0.0-p3"))
        .bind(("t", "2026-04-17T00:00:00Z"))
        .await.expect("seed p3 version").check().expect("seed p3 check");

        // Seed two articles and a related_to edge between them
        db.query(r#"
            CREATE article:a1 CONTENT { store_id: "s1", title: "A1", content: "x",
                source_type: "user", source_id: "", content_hash: "h1", tags: [],
                created_at: "2026-04-17T00:00:00Z", updated_at: "2026-04-17T00:00:00Z" };
            CREATE article:a2 CONTENT { store_id: "s1", title: "A2", content: "y",
                source_type: "user", source_id: "", content_hash: "h2", tags: [],
                created_at: "2026-04-17T01:00:00Z", updated_at: "2026-04-17T01:00:00Z" };
            RELATE article:a1->related_to->article:a2 CONTENT {
                shared_entity_count: 2, strength: 0.5,
                created_at: "2026-04-17T02:00:00Z", updated_at: "2026-04-17T02:00:00Z"
            };
        "#).await.expect("seed").check().expect("seed check");

        db
    }

    #[tokio::test]
    async fn migration_p3_to_p5_renames_related_to_to_entity_overlap() {
        let db = setup_p3_corpus().await;

        // Run migrations: should migrate P3 -> P5
        run_migrations(&db).await.expect("run migrations");

        // After migration: an entity_overlap edge exists with the same payload
        let mut resp = db.query(
            "SELECT meta::id(in) AS from_id, meta::id(out) AS to_id,
                    shared_entity_count, strength, confidence, extraction_method
             FROM entity_overlap"
        ).await.expect("query entity_overlap").check().expect("check");
        #[derive(serde::Deserialize)]
        struct Row { from_id: String, to_id: String, shared_entity_count: i64,
                     strength: f64, confidence: f64, extraction_method: String }
        let rows: Vec<Row> = resp.take(0).unwrap_or_default();

        assert_eq!(rows.len(), 1, "expected exactly one entity_overlap edge");
        assert_eq!(rows[0].from_id, "a1");
        assert_eq!(rows[0].to_id, "a2");
        assert_eq!(rows[0].shared_entity_count, 2);
        assert!((rows[0].strength - 0.5).abs() < 1e-9);
        assert!((rows[0].confidence - 0.5).abs() < 1e-9, "confidence should default to strength");
        assert_eq!(rows[0].extraction_method, "heuristic");

        // And the old related_to table is empty
        let mut resp2 = db.query("SELECT count() AS n FROM related_to GROUP ALL")
            .await.expect("count related_to").check().expect("check2");
        #[derive(serde::Deserialize)] struct Cnt { n: i64 }
        let cnts: Vec<Cnt> = resp2.take(0).unwrap_or_default();
        let n = cnts.first().map(|c| c.n).unwrap_or(0);
        assert_eq!(n, 0, "related_to should be empty after migration");

        // Schema version is now 1.0.0-p7
        let mut resp3 = db.query(
            "SELECT version FROM _schema_version WHERE id = type::thing('_schema_version', 'current')"
        ).await.expect("version").check().expect("check3");
        #[derive(serde::Deserialize)] struct V { version: String }
        let vs: Vec<V> = resp3.take(0).unwrap_or_default();
        assert_eq!(vs.first().map(|v| v.version.as_str()), Some("1.0.0-p7"));
    }

    #[tokio::test]
    async fn migration_p5_to_p5_is_noop() {
        let db = setup_p3_corpus().await;
        run_migrations(&db).await.expect("first run");
        // Second run should be a no-op (entity_overlap stays put, no errors)
        run_migrations(&db).await.expect("second run idempotent");

        let mut resp = db.query("SELECT count() AS n FROM entity_overlap GROUP ALL")
            .await.expect("count").check().expect("check");
        #[derive(serde::Deserialize)] struct Cnt { n: i64 }
        let cnts: Vec<Cnt> = resp.take(0).unwrap_or_default();
        assert_eq!(cnts.first().map(|c| c.n).unwrap_or(0), 1);
    }

    /// Helper: connect to an in-memory SurrealDB and seed it as a P5-state corpus.
    /// Used to verify that the P5 → P7 transition is a clean DDL-only upgrade
    /// (no data migration, just new tables created empty).
    async fn setup_p5_corpus() -> Surreal<Any> {
        let db = connect("memory").await.expect("connect mem");
        db.use_ns("test").use_db("test").await.expect("use ns/db");
        db.query(schema::ddl()).await.expect("ddl").check().expect("ddl check");

        // Pretend schema version is P5
        db.query(
            "UPSERT type::thing('_schema_version', 'current') CONTENT { version: $v, applied_at: $t }"
        )
        .bind(("v", "1.0.0-p5"))
        .bind(("t", "2026-05-23T00:00:00Z"))
        .await.expect("seed p5 version").check().expect("seed p5 check");

        // Seed one article so we can verify it's untouched by the upgrade
        db.query(r#"
            CREATE article:p7m_a1 CONTENT { store_id: "p7m-s1", title: "T", content: "C",
                source_type: "user", source_id: "", content_hash: "p7m-h", tags: [],
                created_at: "2026-05-23T00:00:00Z", updated_at: "2026-05-23T00:00:00Z" };
        "#).await.expect("seed article").check().expect("seed article check");

        db
    }

    #[tokio::test]
    async fn migration_p5_to_p7_is_ddl_only() {
        let db = setup_p5_corpus().await;

        // Run migrations: should transition P5 → P7 via DDL-only (no data touch)
        run_migrations(&db).await.expect("run migrations");

        // Verify: event table exists and is empty
        let mut resp = db.query("SELECT count() AS n FROM event GROUP ALL")
            .await.expect("count event").check().expect("check");
        #[derive(serde::Deserialize)] struct Cnt { n: i64 }
        let cnts: Vec<Cnt> = resp.take(0).unwrap_or_default();
        assert_eq!(cnts.first().map(|c| c.n).unwrap_or(0), 0,
            "event table should exist and be empty");

        // Verify: existing article is untouched
        let mut resp = db.query("SELECT count() AS n FROM article GROUP ALL")
            .await.expect("count article").check().expect("check");
        let cnts: Vec<Cnt> = resp.take(0).unwrap_or_default();
        assert_eq!(cnts.first().map(|c| c.n).unwrap_or(0), 1,
            "existing article should be preserved");

        // Verify: schema version is now P7
        let mut resp = db.query(
            "SELECT version FROM _schema_version WHERE id = type::thing('_schema_version', 'current')"
        ).await.expect("version").check().expect("check");
        #[derive(serde::Deserialize)] struct V { version: String }
        let vs: Vec<V> = resp.take(0).unwrap_or_default();
        assert_eq!(vs.first().map(|v| v.version.as_str()), Some("1.0.0-p7"),
            "schema version should be 1.0.0-p7");
    }

    #[tokio::test]
    async fn migration_p7_to_p7_is_noop() {
        let db = setup_p5_corpus().await;
        run_migrations(&db).await.expect("first run (p5 → p7)");
        // Second run should be a no-op (event table stays empty, no errors)
        run_migrations(&db).await.expect("second run (p7 → p7 idempotent)");

        // Verify: event table still empty (no spontaneous creation)
        let mut resp = db.query("SELECT count() AS n FROM event GROUP ALL")
            .await.expect("count event").check().expect("check");
        #[derive(serde::Deserialize)] struct Cnt { n: i64 }
        let cnts: Vec<Cnt> = resp.take(0).unwrap_or_default();
        assert_eq!(cnts.first().map(|c| c.n).unwrap_or(0), 0,
            "event table should remain empty on second run");
    }
}

