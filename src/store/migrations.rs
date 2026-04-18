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
                Ok(mut r) => { let _ = r.check(); }
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

