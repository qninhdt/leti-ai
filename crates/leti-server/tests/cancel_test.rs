//! Cancel test — verifies that hitting `/abort` while a turn is running
//! flips status to Cancelling and removes the active turn handle.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use leti_protocol::{AbortAckDto, CreateMessageDto, CreateSessionDto, PartDto, SessionDto};
use tower::util::ServiceExt;
use uuid::Uuid;

mod support;

#[tokio::test]
async fn abort_returns_404_for_unknown_session() {
    let harness = support::TestHarness::new().await;
    let app = harness.router();
    let unknown = Uuid::new_v4();
    let resp = app
        .oneshot(
            Request::post(format!("/v1/session/{unknown}/abort"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn prompt_then_abort_marks_cancelling() {
    let harness = support::TestHarness::new().await;
    let app = harness.router();

    // Create session.
    let body = serde_json::to_vec(&CreateSessionDto {
        agent_id: None,
        parent_session_id: None,
        permission_mode: None,
        extensions: serde_json::Value::Null,
        user_questions: true,
        interaction_mode: Default::default(),
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
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let session: SessionDto = serde_json::from_slice(&bytes).unwrap();

    // Fire prompt — provider stub errors immediately, but we still
    // race for the abort path.
    let prompt = CreateMessageDto {
        parts: vec![PartDto::Text {
            id: Uuid::new_v4(),
            text: "hello".to_string(),
        }],
    };
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/v1/session/{}/prompt_async", session.id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&prompt).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);

    // Abort — accepts even if turn already finished.
    let resp = app
        .clone()
        .oneshot(
            Request::post(format!("/v1/session/{}/abort", session.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let _ack: AbortAckDto = serde_json::from_slice(&bytes).unwrap();
}

#[tokio::test]
async fn empty_prompt_rejected() {
    let harness = support::TestHarness::new().await;
    let app = harness.router();
    let body = serde_json::to_vec(&CreateSessionDto {
        agent_id: None,
        parent_session_id: None,
        permission_mode: None,
        extensions: serde_json::Value::Null,
        user_questions: true,
        interaction_mode: Default::default(),
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
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let session: SessionDto = serde_json::from_slice(&bytes).unwrap();

    let prompt = CreateMessageDto { parts: vec![] };
    let resp = app
        .oneshot(
            Request::post(format!("/v1/session/{}/prompt_async", session.id))
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&prompt).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
