//! Integration tests for `POST /v1/session/:id/question/answer`.
//!
//! Auth: route requires an `AuthPrincipal` extension on the request;
//! tests assert the route responds with 401 when none is attached, and
//! 404 once the registry entry is gone (already resolved / unknown id).

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use leti_core::runtime::{QuestionId, QuestionRegistry};
use leti_protocol::dto::QuestionAnswerDto;
use leti_server::{AuthError, AuthPrincipal, Authenticator, RouterBuilder};
use tower::util::ServiceExt;
use uuid::Uuid;

mod support;

/// Authenticator that rejects every request — stands in for "no valid
/// credential presented" so we can exercise the 401 path now that the
/// default router always mounts the admitting dev authenticator.
struct RejectingAuthenticator;

#[async_trait]
impl Authenticator for RejectingAuthenticator {
    async fn authenticate(&self, _headers: &HeaderMap) -> Result<AuthPrincipal, AuthError> {
        Err(AuthError::MissingCredential)
    }
}

#[tokio::test]
async fn question_answer_without_auth_principal_returns_401() {
    let state = support::TestHarness::raw_state().await;
    // Mount a rejecting authenticator so no AuthPrincipal is injected →
    // the auth layer short-circuits with 401 before the route runs.
    let app = RouterBuilder::default().build_with_auth(state, Arc::new(RejectingAuthenticator));

    let body = serde_json::to_vec(&QuestionAnswerDto {
        question_id: Uuid::now_v7(),
        selected: vec![0],
    })
    .expect("serialize");

    let resp = app
        .oneshot(
            Request::post(format!("/v1/session/{}/question/answer", Uuid::now_v7()))
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
    let app = leti_server::build_router(state.clone())
        .layer(axum::Extension(leti_server::AuthPrincipal::user("test")));

    let body = serde_json::to_vec(&QuestionAnswerDto {
        question_id: Uuid::now_v7(),
        selected: vec![0],
    })
    .expect("serialize");

    let resp = app
        .oneshot(
            Request::post(format!("/v1/session/{}/question/answer", Uuid::now_v7()))
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
    let app = leti_server::build_router(state.clone())
        .layer(axum::Extension(leti_server::AuthPrincipal::user("test")));

    // Register a oneshot, hit the route, assert receiver wakes with payload.
    let qid = QuestionId::new();
    let session = leti_core::types::session::SessionId::new();
    let (tx, rx) = tokio::sync::oneshot::channel::<Vec<usize>>();
    registry.register(qid, session, tx);

    let body = serde_json::to_vec(&QuestionAnswerDto {
        question_id: qid.as_uuid(),
        selected: vec![1, 2],
    })
    .expect("serialize");

    let resp = app
        .oneshot(
            Request::post(format!("/v1/session/{}/question/answer", session.as_uuid()))
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("dispatch");
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(rx.await.expect("payload received"), vec![1, 2]);
}
