//! Auth middleware gate (M13/H5): the mounted `AuthLayer` decides access.
//!
//! Proves the layer is actually wired into `build_with_auth`:
//!  - a rejecting authenticator → 401 before any handler runs
//!  - the local dev authenticator → request reaches the handler (200)
//!
//! Uses `GET /v1/health` as a trivial always-mounted target so the test
//! exercises the layer, not route-specific logic.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use openlet_server::{
    AuthError, AuthPrincipal, Authenticator, LocalDevAuthenticator, RouterBuilder,
};
use tower::util::ServiceExt;

mod support;

/// Rejects every request — stands in for an absent/invalid credential.
struct RejectingAuthenticator;

#[async_trait]
impl Authenticator for RejectingAuthenticator {
    async fn authenticate(&self, _headers: &HeaderMap) -> Result<AuthPrincipal, AuthError> {
        Err(AuthError::InvalidCredential("test-reject".into()))
    }
}

#[tokio::test]
async fn rejecting_authenticator_blocks_with_401() {
    let state = support::TestHarness::raw_state().await;
    let app = RouterBuilder::default().build_with_auth(state, Arc::new(RejectingAuthenticator));

    let resp = app
        .oneshot(Request::get("/v1/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "rejected auth must 401 before the handler"
    );
}

#[tokio::test]
async fn local_dev_authenticator_admits_request() {
    let state = support::TestHarness::raw_state().await;
    let app =
        RouterBuilder::default().build_with_auth(state, Arc::new(LocalDevAuthenticator::default()));

    let resp = app
        .oneshot(Request::get("/v1/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "dev authenticator must admit the request to the handler"
    );
}

/// Default `build()` uses the dev authenticator, so the local binary
/// keeps working end-to-end without any auth configuration.
#[tokio::test]
async fn default_build_admits_request() {
    let state = support::TestHarness::raw_state().await;
    let app = RouterBuilder::default().build(state);

    let resp = app
        .oneshot(Request::get("/v1/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
