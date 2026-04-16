//! End-to-end migration integration test.

use anyhow::Result;
use knowledge_nexus_agent::migrate::{self, is_migrated, migration_marker_path};
use knowledge_nexus_agent::store::{Store, SurrealStore};
use rusqlite::Connection;
use tempfile::TempDir;

fn seed_legacy_sqlite(path: &std::path::Path) -> Result<()> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA foreign_keys = ON;")?;

    conn.execute_batch(
        "CREATE TABLE users (
            id TEXT PRIMARY KEY, username TEXT UNIQUE, display_name TEXT,
            is_owner INTEGER, settings TEXT, created_at TEXT, updated_at TEXT
         );
         CREATE TABLE knowledge_stores (
            id TEXT PRIMARY KEY, owner_id TEXT, store_type TEXT, name TEXT,
            lancedb_collection TEXT, created_at TEXT, updated_at TEXT
         );
         CREATE TABLE articles (
            id TEXT PRIMARY KEY, store_id TEXT, title TEXT, content TEXT,
            source_type TEXT, tags TEXT, embedded_at TEXT, created_at TEXT, updated_at TEXT
         );
         CREATE TABLE conversations (
            id TEXT PRIMARY KEY, user_id TEXT, title TEXT,
            message_count INTEGER, created_at TEXT, updated_at TEXT
         );
         CREATE TABLE messages (
            id TEXT PRIMARY KEY, conversation_id TEXT, role TEXT,
            content TEXT, metadata TEXT, created_at TEXT
         );
         CREATE TABLE k2k_clients (
            client_id TEXT PRIMARY KEY, public_key_pem TEXT,
            client_name TEXT, registered_at TEXT, status TEXT
         );
         CREATE TABLE federation_agreements (
            id TEXT PRIMARY KEY, local_store_id TEXT, remote_node_id TEXT,
            remote_endpoint TEXT, access_type TEXT, created_at TEXT
         );
         CREATE TABLE discovered_nodes (
            node_id TEXT PRIMARY KEY, host TEXT, port INTEGER, endpoint TEXT,
            capabilities TEXT, last_seen TEXT, healthy INTEGER
         );
         CREATE TABLE connector_configs (
            id TEXT PRIMARY KEY, connector_type TEXT, name TEXT, config TEXT,
            store_id TEXT, created_at TEXT, updated_at TEXT
         );",
    )?;

    let ts = "2026-04-15T00:00:00Z";
    conn.execute(
        "INSERT INTO users VALUES (?,?,?,?,?,?,?)",
        rusqlite::params!["u1", "alice", "Alice", 1, "{}", ts, ts],
    )?;
    conn.execute(
        "INSERT INTO knowledge_stores VALUES (?,?,?,?,?,?,?)",
        rusqlite::params!["s1", "u1", "personal", "Notes", "store_s1", ts, ts],
    )?;
    conn.execute(
        "INSERT INTO articles VALUES (?,?,?,?,?,?,?,?,?)",
        rusqlite::params![
            "a1",
            "s1",
            "Rust ownership",
            "The borrow checker enforces rules.",
            "user",
            r#"["rust","ownership"]"#,
            ts,
            ts,
            ts
        ],
    )?;
    conn.execute(
        "INSERT INTO conversations VALUES (?,?,?,?,?,?)",
        rusqlite::params!["c1", "u1", "Chat", 1, ts, ts],
    )?;
    conn.execute(
        "INSERT INTO messages VALUES (?,?,?,?,?,?)",
        rusqlite::params!["m1", "c1", "user", "Hi", "{}", ts],
    )?;
    conn.execute(
        "INSERT INTO k2k_clients VALUES (?,?,?,?,?)",
        rusqlite::params!["client1", "PEM", "Tester", ts, "approved"],
    )?;
    conn.execute(
        "INSERT INTO federation_agreements VALUES (?,?,?,?,?,?)",
        rusqlite::params!["fa1", "s1", "node2", "http://n2/k2k", "read", ts],
    )?;
    conn.execute(
        "INSERT INTO discovered_nodes VALUES (?,?,?,?,?,?,?)",
        rusqlite::params!["node2", "192.168.1.20", 8765, "http://n2/k2k", "[]", ts, 1],
    )?;
    conn.execute(
        "INSERT INTO connector_configs VALUES (?,?,?,?,?,?,?)",
        rusqlite::params!["cc1", "local_files", "Docs", r#"{"path":"/tmp"}"#, "s1", ts, ts],
    )?;
    Ok(())
}

#[tokio::test]
async fn test_full_migration_round_trips_every_table() {
    let tmp = TempDir::new().unwrap();
    let sqlite_path = tmp.path().join("legacy.db");
    let surreal_dir = tmp.path().join("surreal");

    seed_legacy_sqlite(&sqlite_path).unwrap();

    let report = migrate::migrate(&sqlite_path, &surreal_dir, false)
        .await
        .expect("migration succeeds");
    assert_eq!(report.users, 1);
    assert_eq!(report.stores, 1);
    assert_eq!(report.articles, 1);
    assert_eq!(report.conversations, 1);
    assert_eq!(report.messages, 1);
    assert_eq!(report.k2k_clients, 1);
    assert_eq!(report.federation_agreements, 1);
    assert_eq!(report.discovered_nodes, 1);
    assert_eq!(report.connector_configs, 1);
    assert!(is_migrated(&surreal_dir));

    let store = SurrealStore::open(&surreal_dir).await.unwrap();

    // User
    let user = store.get_user("u1").await.unwrap().expect("u1");
    assert_eq!(user.username, "alice");
    assert!(user.is_owner);

    // Store (new field defaulted)
    let ks = store.get_store("s1").await.unwrap().expect("s1");
    assert_eq!(ks.quantizer_version, "ivf_pq_v1");

    // Article (content_hash backfilled, source_id empty, tags preserved)
    let article = store.get_article("a1").await.unwrap().expect("a1");
    assert_eq!(article.content_hash.len(), 64);
    assert_eq!(article.source_id, "");
    assert_eq!(article.tags, serde_json::json!(["rust", "ownership"]));

    // Hash lookup
    let lookup = store
        .find_article_by_hash("s1", &article.content_hash)
        .await
        .unwrap()
        .expect("hash lookup finds the article");
    assert_eq!(lookup.id, "a1");

    // Conversation preserves message_count from legacy
    let conv = store.get_conversation("c1").await.unwrap().expect("c1");
    assert_eq!(conv.message_count, 1);

    // K2K client
    assert!(store.get_k2k_client("client1").await.unwrap().is_some());

    // Federation
    assert_eq!(store.list_federation_agreements().await.unwrap().len(), 1);
    assert_eq!(store.list_discovered_nodes().await.unwrap().len(), 1);

    // Connectors
    assert_eq!(store.list_connector_configs().await.unwrap().len(), 1);
}

#[tokio::test]
async fn test_migration_refuses_to_rerun_without_force() {
    let tmp = TempDir::new().unwrap();
    let sqlite_path = tmp.path().join("legacy.db");
    let surreal_dir = tmp.path().join("surreal");
    seed_legacy_sqlite(&sqlite_path).unwrap();

    migrate::migrate(&sqlite_path, &surreal_dir, false)
        .await
        .unwrap();

    let err = migrate::migrate(&sqlite_path, &surreal_dir, false)
        .await
        .expect_err("second run without --force should fail");
    assert!(err.to_string().contains("migration_completed"));
}

#[tokio::test]
async fn test_migration_force_reruns_idempotently() {
    let tmp = TempDir::new().unwrap();
    let sqlite_path = tmp.path().join("legacy.db");
    let surreal_dir = tmp.path().join("surreal");
    seed_legacy_sqlite(&sqlite_path).unwrap();

    migrate::migrate(&sqlite_path, &surreal_dir, false)
        .await
        .unwrap();
    std::fs::remove_file(migration_marker_path(&surreal_dir)).unwrap();

    let report = migrate::migrate(&sqlite_path, &surreal_dir, true)
        .await
        .unwrap();
    assert_eq!(report.articles, 1);

    let store = SurrealStore::open(&surreal_dir).await.unwrap();
    assert_eq!(
        store.list_articles_for_store("s1").await.unwrap().len(),
        1,
        "upsert must not duplicate rows"
    );
}
