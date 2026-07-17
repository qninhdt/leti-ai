//! Smoke test for `tests/common/` helpers in `leti-adapters`.

mod common;

use common::sqlite_helper::make_pool;
use common::tempdir_workspace::WorkspaceFixture;
use common::wiremock_helpers::{mount_openai_chat_stream, mount_rate_limited, mount_status_only};

#[tokio::test]
async fn make_pool_runs_migrations_idempotently() {
    let pool = make_pool().await;
    // Re-running migrations on an already-migrated pool must succeed.
    leti_adapters::sqlite::run_migrations(&pool)
        .await
        .expect("re-run migrations no-op");
}

#[test]
fn workspace_fixture_seeds_relative_paths() {
    let fx = WorkspaceFixture::with_files(vec![("a/b.txt", "hello"), ("c.txt", "world")]);
    let a = std::fs::read_to_string(fx.root().join("a/b.txt")).unwrap();
    let c = std::fs::read_to_string(fx.root().join("c.txt")).unwrap();
    assert_eq!(a, "hello");
    assert_eq!(c, "world");
}

#[test]
fn workspace_fixture_empty_creates_root() {
    let fx = WorkspaceFixture::empty();
    assert!(fx.root().is_dir());
}

#[tokio::test]
async fn wiremock_chat_stream_returns_canned_sse_body() {
    let server = wiremock::MockServer::start().await;
    mount_openai_chat_stream(&server, &[r#"{"choices":[{"delta":{"content":"hi"}}]}"#]).await;

    let resp = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", server.uri()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains(r#""content":"hi""#));
    assert!(body.trim_end().ends_with("data: [DONE]"));
}

#[tokio::test]
async fn wiremock_status_only_round_trips() {
    let server = wiremock::MockServer::start().await;
    mount_status_only(&server, 503).await;
    let resp = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", server.uri()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 503);
}

#[tokio::test]
async fn wiremock_rate_limited_emits_retry_after_header() {
    let server = wiremock::MockServer::start().await;
    mount_rate_limited(&server, 7).await;
    let resp = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", server.uri()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 429);
    assert_eq!(
        resp.headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok()),
        Some("7")
    );
}
