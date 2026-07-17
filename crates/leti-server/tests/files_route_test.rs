//! `/v1/files*` integration — real workspace walk + jailed content read.
//!
//! Boots the default router over a temp workspace seeded with real files
//! (including a `.env` secret) and asserts: the listing surfaces real
//! files and excludes secrets, content reads return real bytes, and the
//! path guard rejects absolute / traversal requests.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::util::ServiceExt;

mod support;

/// Seed the workspace, mount the default router, return (router, tempdir).
/// The tempdir guard must outlive the router so the workspace stays on disk.
async fn harness_with_files() -> (axum::Router, tempfile::TempDir) {
    let (state, tempdir) = support::TestHarness::build_state().await;
    let ws = state.workspace_root.clone();

    tokio::fs::create_dir_all(ws.join("src")).await.unwrap();
    tokio::fs::write(ws.join("src/app.rs"), b"fn main() {}\n")
        .await
        .unwrap();
    tokio::fs::write(ws.join("README.md"), b"# Hello\nreal content\n")
        .await
        .unwrap();
    tokio::fs::write(ws.join(".env"), b"SECRET=shhh\n")
        .await
        .unwrap();

    let app = leti_server::build_router(state);
    (app, tempdir)
}

async fn get_json(app: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn lists_real_files_and_excludes_secrets() {
    let (app, _tempdir) = harness_with_files().await;

    let (status, body) = get_json(&app, "/v1/files").await;
    assert_eq!(status, StatusCode::OK);

    let paths: Vec<String> = body["files"]
        .as_array()
        .expect("files array")
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_string())
        .collect();

    assert!(paths.contains(&"src/app.rs".to_string()), "got {paths:?}");
    assert!(paths.contains(&"README.md".to_string()), "got {paths:?}");
    assert!(
        !paths.iter().any(|p| p.ends_with(".env")),
        ".env must be excluded; got {paths:?}"
    );
}

#[tokio::test]
async fn query_filters_listing() {
    let (app, _tempdir) = harness_with_files().await;

    let (status, body) = get_json(&app, "/v1/files?query=readme").await;
    assert_eq!(status, StatusCode::OK);
    let paths: Vec<String> = body["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(paths, vec!["README.md".to_string()], "got {paths:?}");
}

#[tokio::test]
async fn reads_real_content() {
    let (app, _tempdir) = harness_with_files().await;

    let (status, body) = get_json(&app, "/v1/files/content?path=README.md").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["type"], "text");
    assert_eq!(body["content"], "# Hello\nreal content\n");
}

#[tokio::test]
async fn content_rejects_traversal() {
    let (app, _tempdir) = harness_with_files().await;

    let (status, _body) = get_json(&app, "/v1/files/content?path=../etc/passwd").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn content_rejects_absolute() {
    let (app, _tempdir) = harness_with_files().await;

    let (status, _body) = get_json(&app, "/v1/files/content?path=/etc/passwd").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn content_secret_path_is_not_found() {
    let (app, _tempdir) = harness_with_files().await;

    // .env exists on disk but the secret filter masks it as 404 so its
    // existence is never confirmed through the @-mention surface.
    let (status, _body) = get_json(&app, "/v1/files/content?path=.env").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn content_missing_file_is_not_found() {
    let (app, _tempdir) = harness_with_files().await;

    let (status, _body) = get_json(&app, "/v1/files/content?path=does/not/exist.rs").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
