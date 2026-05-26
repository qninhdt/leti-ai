//! Workspace routing middleware tests — F5.1 cross-tenant isolation.
//!
//! Three scenarios lock the auth-ordering contract:
//!  1. Mounted without injected `AuthPrincipal` → 401
//!  2. Mounted with principal + valid workspace → 200
//!  3. Mounted with principal + missing workspace → 404

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Extension, Router};
use openlet_server::middleware::{
    AuthPrincipal, WORKSPACE_HEADER, WorkspaceRoutingGuard, WorkspaceRoutingLayer,
};
use openlet_server::workspace_resolver::{WorkspaceError, WorkspaceResolver};
use openlet_server::{AppState, StaticWorkspaceResolver};
use tower::ServiceExt;

mod support;

/// Resolver that 404s for the magic id `missing` and resolves the
/// fixed harness state for everything else.
struct ConditionalResolver {
    state: Arc<AppState>,
}

#[async_trait]
impl WorkspaceResolver for ConditionalResolver {
    async fn resolve(&self, workspace_id: &str) -> Result<Arc<AppState>, WorkspaceError> {
        if workspace_id == "missing" {
            return Err(WorkspaceError::NotFound(workspace_id.to_string()));
        }
        Ok(self.state.clone())
    }
}

async fn handler(_guard: WorkspaceRoutingGuard) -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

/// Build a small router with workspace routing layered on top. When
/// `inject_principal` is true, an upstream middleware injects an
/// `AuthPrincipal` extension so the downstream layer can succeed.
fn build_router(state: Arc<AppState>, inject_principal: bool) -> Router {
    let resolver = StaticWorkspaceResolver::new(state.clone());
    let route = Router::new()
        .route("/v1/test", get(handler))
        .layer(WorkspaceRoutingLayer::new(resolver));
    if inject_principal {
        route.layer(from_fn(
            |mut req: Request<Body>, next: axum::middleware::Next| async move {
                req.extensions_mut().insert(AuthPrincipal {
                    subject: "test-user".into(),
                });
                next.run(req).await
            },
        ))
    } else {
        route
    }
}

#[tokio::test]
async fn missing_principal_returns_401() {
    let state = Arc::new(support::TestHarness::raw_state().await);
    let app = build_router(state, false);
    let resp = app
        .oneshot(
            Request::get("/v1/test")
                .header(WORKSPACE_HEADER, "default")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "no AuthPrincipal in extensions must yield 401"
    );
}

#[tokio::test]
async fn with_principal_and_valid_workspace_returns_200() {
    let state = Arc::new(support::TestHarness::raw_state().await);
    let app = build_router(state, true);
    let resp = app
        .oneshot(
            Request::get("/v1/test")
                .header(WORKSPACE_HEADER, "default")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn missing_workspace_returns_404() {
    let state = Arc::new(support::TestHarness::raw_state().await);
    let resolver = ConditionalResolver {
        state: state.clone(),
    };
    let route = Router::new()
        .route("/v1/test", get(handler))
        .layer(WorkspaceRoutingLayer::new(resolver))
        .layer(from_fn(
            |mut req: Request<Body>, next: axum::middleware::Next| async move {
                req.extensions_mut().insert(AuthPrincipal {
                    subject: "test-user".into(),
                });
                next.run(req).await
            },
        ));

    let resp = route
        .oneshot(
            Request::get("/v1/test")
                .header(WORKSPACE_HEADER, "missing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn extension_unused_compiles() {
    // Confirms the `Extension<Arc<AppState>>` extractor works with the
    // request extensions populated by the layer; downstream handlers
    // that don't want the guard can still pull state.
    async fn alt_handler(Extension(_state): Extension<Arc<AppState>>) -> impl IntoResponse {
        (StatusCode::OK, "alt")
    }
    let state = Arc::new(support::TestHarness::raw_state().await);
    let resolver = StaticWorkspaceResolver::new(state.clone());
    let app = Router::new()
        .route("/v1/alt", get(alt_handler))
        .layer(WorkspaceRoutingLayer::new(resolver))
        .layer(from_fn(
            |mut req: Request<Body>, next: axum::middleware::Next| async move {
                req.extensions_mut().insert(AuthPrincipal {
                    subject: "test-user".into(),
                });
                next.run(req).await
            },
        ));
    let resp = app
        .oneshot(
            Request::get("/v1/alt")
                .header(WORKSPACE_HEADER, "default")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
