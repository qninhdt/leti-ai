//! Tests for `LocalFsArtifactStore` — round-trip + path traversal rejection.

use bytes::Bytes;
use openlet_adapters::localfs::LocalFsArtifactStore;
use openlet_adapters::sqlite::open_in_memory;
use openlet_adapters::sqlite::SqliteMemoryStore;
use openlet_core::adapters::artifact_store::{ArtifactRef, ArtifactStore};
use openlet_core::adapters::memory_store::MemoryStore;

#[tokio::test]
async fn put_then_get_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let pool = open_in_memory().await.unwrap();
    let store = LocalFsArtifactStore::new(dir.path().to_path_buf(), pool.clone());
    let mem = SqliteMemoryStore::new(pool);
    let session = mem.create_session("a", None).await.unwrap();

    let payload = Bytes::from_static(b"hello world");
    let r = store
        .put(session, "report.txt", payload.clone())
        .await
        .expect("put");
    assert_eq!(r.size, payload.len() as u64);
    assert_eq!(r.key, "report.txt");

    let fetched = store.get(&r).await.expect("get");
    assert_eq!(fetched, payload);
}

#[tokio::test]
async fn list_returns_session_keys() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_in_memory().await.unwrap();
    let store = LocalFsArtifactStore::new(dir.path().to_path_buf(), pool.clone());
    let mem = SqliteMemoryStore::new(pool);
    let session = mem.create_session("a", None).await.unwrap();

    store.put(session, "a.txt", Bytes::from_static(b"a")).await.unwrap();
    store.put(session, "b.txt", Bytes::from_static(b"bb")).await.unwrap();

    let listed = store.list(session).await.unwrap();
    assert_eq!(listed.len(), 2);
    let keys: Vec<&str> = listed.iter().map(|r| r.key.as_str()).collect();
    assert!(keys.contains(&"a.txt"));
    assert!(keys.contains(&"b.txt"));
}

#[tokio::test]
async fn rejects_traversal_keys() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_in_memory().await.unwrap();
    let store = LocalFsArtifactStore::new(dir.path().to_path_buf(), pool.clone());
    let mem = SqliteMemoryStore::new(pool);
    let session = mem.create_session("a", None).await.unwrap();

    let bad_keys = ["../etc/passwd", "/etc/passwd", "..", "..\\..\\evil"];
    for k in bad_keys {
        let res = store
            .put(session, k, Bytes::from_static(b"x"))
            .await;
        assert!(res.is_err(), "key {k} should be rejected");
    }
}

#[tokio::test]
async fn get_missing_returns_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let pool = open_in_memory().await.unwrap();
    let store = LocalFsArtifactStore::new(dir.path().to_path_buf(), pool.clone());
    let mem = SqliteMemoryStore::new(pool);
    let session = mem.create_session("a", None).await.unwrap();

    let r = ArtifactRef {
        session_id: session,
        key: "missing.txt".into(),
        size: 0,
        mime: None,
    };
    let err = store.get(&r).await.unwrap_err();
    matches!(err, openlet_core::error::ArtifactError::NotFound(_));
}
