//! Integration test for session CRUD routes.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use openlet_protocol::{CreateSessionDto, SessionDto};
use serde_json::json;
use tower::util::ServiceExt;

mod support;

#[tokio::test]
async fn create_list_get_delete_session_round_trip() {
    let harness = support::TestHarness::new().await;
    let app = harness.router();

    let body = serde_json::to_vec(&CreateSessionDto {
        agent_id: None,
        parent_session_id: None,
        permission_mode: None,
        extensions: serde_json::Value::Null,
    })
    .unwrap();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/v1/session")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let session: SessionDto = serde_json::from_slice(&bytes).unwrap();

    // GET /session
    let resp = app
        .clone()
        .oneshot(Request::get("/v1/session").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let listed: Vec<SessionDto> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, session.id);

    // GET /session/:id
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/v1/session/{}", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // DELETE /session/:id
    let resp = app
        .clone()
        .oneshot(
            Request::delete(format!("/v1/session/{}", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn unknown_session_returns_404() {
    let harness = support::TestHarness::new().await;
    let app = harness.router();
    let unknown = uuid::Uuid::new_v4();
    let resp = app
        .oneshot(
            Request::get(format!("/v1/session/{unknown}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn list_agents_includes_default() {
    let harness = support::TestHarness::new().await;
    let app = harness.router();
    let resp = app
        .oneshot(Request::get("/v1/agent").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let agents: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let arr = agents.as_array().expect("array");
    assert_eq!(arr.len(), 1);
    assert!(arr[0].get("id").is_some());
}

#[tokio::test]
async fn permission_reply_unknown_ask_returns_404() {
    let harness = support::TestHarness::new().await;
    let app = harness.router();
    let body = json!({"decision": "allow"});
    let unknown = uuid::Uuid::new_v4();
    let resp = app
        .oneshot(
            Request::post(format!("/v1/permission/{unknown}"))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// `POST /v1/session` with `extensions` echoes the blob back via
/// `GET /v1/session/:id`. Confirms slice 4-E round-trip — the auth
/// blob the integrator sends survives the SQLite layer untouched.
#[tokio::test]
async fn create_session_with_extensions_round_trip() {
    let harness = support::TestHarness::new().await;
    let app = harness.router();
    let extensions = json!({
        "user_id": "u_123",
        "tenant_id": "t_42",
        "scopes": ["read", "write"],
    });
    let body = serde_json::to_vec(&CreateSessionDto {
        agent_id: None,
        parent_session_id: None,
        permission_mode: None,
        extensions: extensions.clone(),
    })
    .unwrap();
    let resp = app
        .clone()
        .oneshot(
            Request::post("/v1/session")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let created: SessionDto = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(created.extensions, extensions);

    let resp = app
        .oneshot(
            Request::get(format!("/v1/session/{}", created.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let fetched: SessionDto = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(fetched.extensions, extensions);
}
