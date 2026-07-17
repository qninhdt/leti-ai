//! Integration tests for `LocalFsArtifactStore`.
//!
//! Locks the contract:
//! 1. put + get round-trip preserves bytes; ArtifactRef carries the size.
//! 2. validate_key rejects empty / `..` / leading-slash keys before any
//!    filesystem interaction (defense against `../../etc/passwd` attacks).
//! 3. list returns artifacts in `created_at ASC` order, scoped per session.
//! 4. put on the same (session, key) overwrites bytes + size + timestamp
//!    (UPSERT-style).
//! 5. Concurrent put of the same (session, key) bytes results in exactly
//!    ONE on-disk file (sha256-keyed path is deterministic) and ONE row
//!    in the artifacts table.
//! 6. get of an unknown key returns `ArtifactError::NotFound(key)`.

mod common;

use std::sync::Arc;

use bytes::Bytes;
use common::sqlite_helper::make_pool;
use leti_adapters::localfs::LocalFsArtifactStore;
use leti_adapters::sqlite::SqliteMemoryStore;
use leti_core::adapters::artifact_store::{ArtifactRef, ArtifactStore};
use leti_core::adapters::memory_store::MemoryStore;
use leti_core::error::ArtifactError;
use leti_core::types::agent::AgentId;
use leti_core::types::session::SessionId;
use sqlx::Row;
use tempfile::TempDir;

async fn setup() -> (
    LocalFsArtifactStore,
    SqliteMemoryStore,
    TempDir,
    sqlx::SqlitePool,
) {
    let pool = make_pool().await;
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = LocalFsArtifactStore::new(tmp.path().to_path_buf(), pool.clone());
    let mem = SqliteMemoryStore::new(pool.clone());
    (store, mem, tmp, pool)
}

async fn fresh_session(mem: &SqliteMemoryStore) -> SessionId {
    mem.create_session(AgentId::new(), None).await.unwrap()
}

#[tokio::test]
async fn put_and_get_round_trips_bytes() {
    let (store, mem, _tmp, _pool) = setup().await;
    let session = fresh_session(&mem).await;
    let payload = Bytes::from_static(b"hello artifact");
    let r = store
        .put(session, "blob.bin", payload.clone())
        .await
        .unwrap();
    assert_eq!(r.size, payload.len() as u64);
    assert_eq!(r.key, "blob.bin");

    let got = store.get(&r).await.unwrap();
    assert_eq!(&got[..], &payload[..]);
}

#[tokio::test]
async fn list_returns_artifacts_scoped_per_session_in_creation_order() {
    let (store, mem, _tmp, _pool) = setup().await;
    let session_a = fresh_session(&mem).await;
    let session_b = fresh_session(&mem).await;

    store
        .put(session_a, "a1", Bytes::from_static(b"1"))
        .await
        .unwrap();
    store
        .put(session_a, "a2", Bytes::from_static(b"22"))
        .await
        .unwrap();
    store
        .put(session_b, "b1", Bytes::from_static(b"x"))
        .await
        .unwrap();

    let listed_a = store.list(session_a).await.unwrap();
    let keys_a: Vec<_> = listed_a.iter().map(|r| r.key.as_str()).collect();
    assert_eq!(keys_a, vec!["a1", "a2"], "list scoped to session_a");

    let listed_b = store.list(session_b).await.unwrap();
    let keys_b: Vec<_> = listed_b.iter().map(|r| r.key.as_str()).collect();
    assert_eq!(keys_b, vec!["b1"], "list scoped to session_b");
}

#[tokio::test]
async fn put_overwrites_existing_key_with_new_bytes() {
    let (store, mem, _tmp, pool) = setup().await;
    let session = fresh_session(&mem).await;
    let r1 = store
        .put(session, "k", Bytes::from_static(b"old"))
        .await
        .unwrap();
    let r2 = store
        .put(session, "k", Bytes::from_static(b"newer-bytes"))
        .await
        .unwrap();
    assert_eq!(r1.key, r2.key);
    assert_eq!(r2.size, "newer-bytes".len() as u64);

    let got = store.get(&r2).await.unwrap();
    assert_eq!(&got[..], b"newer-bytes");

    // Exactly one row in the artifacts table for (session, key).
    let count: i64 =
        sqlx::query("SELECT COUNT(*) AS c FROM artifacts WHERE session_id = ? AND key = ?")
            .bind(session.to_string())
            .bind("k")
            .fetch_one(&pool)
            .await
            .unwrap()
            .try_get("c")
            .unwrap();
    assert_eq!(
        count, 1,
        "UPSERT must keep exactly one row per (session, key)"
    );
}

#[tokio::test]
async fn concurrent_put_same_key_results_in_single_row() {
    let (store, mem, _tmp, pool) = setup().await;
    let store = Arc::new(store);
    let session = fresh_session(&mem).await;

    let mut handles = Vec::new();
    for i in 0..8 {
        let store = store.clone();
        let payload = Bytes::from(format!("payload-{i}"));
        handles.push(tokio::spawn(async move {
            store.put(session, "shared-key", payload).await
        }));
    }
    for h in handles {
        h.await.unwrap().expect("put");
    }

    let count: i64 =
        sqlx::query("SELECT COUNT(*) AS c FROM artifacts WHERE session_id = ? AND key = ?")
            .bind(session.to_string())
            .bind("shared-key")
            .fetch_one(&pool)
            .await
            .unwrap()
            .try_get("c")
            .unwrap();
    assert_eq!(
        count, 1,
        "8 concurrent puts must collapse to 1 artifacts row"
    );

    // The on-disk path is sha256(key)-derived → all 8 writes hit the
    // same file. After the dust settles, the file must be readable.
    let listed = store.list(session).await.unwrap();
    assert_eq!(listed.len(), 1);
    let bytes = store.get(&listed[0]).await.unwrap();
    assert!(
        bytes.starts_with(b"payload-"),
        "final bytes must be one of the puts"
    );
}

#[tokio::test]
async fn validate_key_rejects_empty_traversal_and_absolute_paths() {
    let (store, mem, _tmp, _pool) = setup().await;
    let session = fresh_session(&mem).await;

    let bad_keys = [
        "",
        "..",
        "../etc/passwd",
        "/abs/path",
        "\\windows\\path",
        "ok/../escape",
    ];
    for key in bad_keys {
        let err = store
            .put(session, key, Bytes::from_static(b"x"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ArtifactError::Io(_)),
            "key {key:?} must be rejected, got {err:?}"
        );
    }
}

#[tokio::test]
async fn get_unknown_key_returns_not_found() {
    let (store, mem, _tmp, _pool) = setup().await;
    let session = fresh_session(&mem).await;
    let phantom = ArtifactRef {
        session_id: session,
        key: "never-put".to_string(),
        size: 0,
        mime: None,
    };
    let err = store.get(&phantom).await.unwrap_err();
    assert!(
        matches!(&err, ArtifactError::NotFound(k) if k == "never-put"),
        "missing key must surface NotFound with the key, got {err:?}"
    );
}

#[tokio::test]
async fn put_isolates_bytes_per_session_for_same_key() {
    // Two sessions can both put under key "config.json" without
    // colliding — sha256(key) gives the filename, but session_dir
    // namespaces the parent directory.
    let (store, mem, _tmp, _pool) = setup().await;
    let s1 = fresh_session(&mem).await;
    let s2 = fresh_session(&mem).await;
    store
        .put(s1, "config.json", Bytes::from_static(b"alpha"))
        .await
        .unwrap();
    store
        .put(s2, "config.json", Bytes::from_static(b"beta"))
        .await
        .unwrap();

    let r1 = ArtifactRef {
        session_id: s1,
        key: "config.json".to_string(),
        size: 5,
        mime: None,
    };
    let r2 = ArtifactRef {
        session_id: s2,
        key: "config.json".to_string(),
        size: 4,
        mime: None,
    };
    assert_eq!(&store.get(&r1).await.unwrap()[..], b"alpha");
    assert_eq!(&store.get(&r2).await.unwrap()[..], b"beta");
}
