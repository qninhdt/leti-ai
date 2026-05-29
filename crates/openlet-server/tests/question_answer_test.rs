//! Integration tests for `POST /v1/sessions/:id/question/answer`.
//!
//! Auth: route requires an `AuthPrincipal` extension on the request;
//! tests assert the route responds with 401 when none is attached, and
//! 404 once the registry entry is gone (already resolved / unknown id).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use openlet_core::runtime::{QuestionId, QuestionRegistry};
use openlet_protocol::dto::QuestionAnswerDto;
use tower::util::ServiceExt;
use uuid::Uuid;

mod support;

#[tokio::test]
async fn question_answer_without_auth_principal_returns_401() {
    let harness = support::TestHarness::new().await;
    let app = harness.router();

    let body = serde_json::to_vec(&QuestionAnswerDto {
        question_id: Uuid::now_v7(),
        selected: vec![0],
    })
    .expect("serialize");

    let resp = app
        .oneshot(
            Request::post(format!("/v1/sessions/{}/question/answer", Uuid::now_v7()))
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("dispatch");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn question_answer_unknown_id_with_auth_returns_404() {
    let state = support::TestHarness::raw_state().await;
    let app = openlet_server::build_router(state.clone()).layer(axum::Extension(
        openlet_server::routes::question::AuthPrincipal,
    ));

    let body = serde_json::to_vec(&QuestionAnswerDto {
        question_id: Uuid::now_v7(),
        selected: vec![0],
    })
    .expect("serialize");

    let resp = app
        .oneshot(
            Request::post(format!("/v1/sessions/{}/question/answer", Uuid::now_v7()))
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("dispatch");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn question_answer_resolves_registered_id() {
    let state = support::TestHarness::raw_state().await;
    let registry: Arc<QuestionRegistry> = state.questions.clone();
    let app = openlet_server::build_router(state.clone()).layer(axum::Extension(
        openlet_server::routes::question::AuthPrincipal,
    ));

    // Register a oneshot, hit the route, assert receiver wakes with payload.
    let qid = QuestionId::new();
    let session = openlet_core::types::session::SessionId::new();
    let (tx, rx) = tokio::sync::oneshot::channel::<Vec<usize>>();
    registry.register(qid, session, tx);

    let body = serde_json::to_vec(&QuestionAnswerDto {
        question_id: qid.as_uuid(),
        selected: vec![1, 2],
    })
    .expect("serialize");

    let resp = app
        .oneshot(
            Request::post(format!(
                "/v1/sessions/{}/question/answer",
                session.as_uuid()
            ))
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap(),
        )
        .await
        .expect("dispatch");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(rx.await.expect("payload received"), vec![1, 2]);
}
