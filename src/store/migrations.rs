//! Schema version tracking for SurrealDB.
//!
//! On every `SurrealStore::open`, we run `schema::ddl()` (which is idempotent),
//! then record the version in the `_schema_version` table. Future phases
//! (P3 graph, P5 quantizer fields) will append new DDL blocks and bump
//! `SCHEMA_VERSION` — each past version's DDL stays in this file forever.

use anyhow::{Context, Result};
use surrealdb::engine::any::Any;
use surrealdb::Surreal;

use super::schema;

pub async fn run_migrations(db: &Surreal<Any>) -> Result<()> {
    db.query(schema::ddl())
        .await
        .context("Failed to apply SurrealDB DDL")?
        .check()
        .context("SurrealDB DDL returned an error")?;

    let applied_at = chrono::Utc::now().to_rfc3339();
    db.query(
        "CREATE _schema_version CONTENT { version: $version, applied_at: $applied_at }",
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
