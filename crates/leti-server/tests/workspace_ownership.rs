//! C2 regression: workspace ownership enforcement (403).
//!
//! The single-tenant `StaticWorkspaceResolver` can't exercise a
//! cross-tenant mismatch (one owner, always resolves). This test uses a
//! 2-tenant resolver that maps each workspace id to an owner principal
//! and returns `WorkspaceError::Forbidden` when the caller isn't the
//! owner — proving the layer maps that to a 403 and that the principal
//! actually reaches the resolver.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Extension;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::response::IntoResponse;
use axum::routing::get;
use leti_server::WORKSPACE_HEADER;
use leti_server::workspace_resolver::{WorkspaceError, WorkspaceResolver};
use leti_server::{AppState, AuthPrincipal, WorkspaceRoutingLayer};
use tower::ServiceExt;

mod support;

/// Two-tenant resolver: `workspace_id → owner caller_id`. Resolves the
/// shared harness state only when the request principal owns the target
/// workspace; otherwise 403. Mirrors the cloud ownership gate.
struct TwoTenantResolver {
    state: Arc<AppState>,
    owners: HashMap<String, String>,
}

#[async_trait]
impl WorkspaceResolver for TwoTenantResolver {
    async fn resolve(
        &self,
        principal: &AuthPrincipal,
        workspace_id: &str,
    ) -> Result<Arc<AppState>, WorkspaceError> {
        match self.owners.get(workspace_id) {
            None => Err(WorkspaceError::NotFound(workspace_id.to_string())),
            Some(owner) if owner == &principal.caller_id => Ok(self.state.clone()),
            Some(_) => Err(WorkspaceError::Forbidden(format!(
                "caller {} does not own workspace {workspace_id}",
                principal.caller_id
            ))),
        }
    }
}

async fn handler(Extension(_state): Extension<Arc<AppState>>) -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

/// Build a router whose injected principal is `caller_id`, resolving
/// through a 2-tenant resolver where `alice` owns `ws-alice` and `bob`
/// owns `ws-bob`.
fn app_for(state: Arc<AppState>, caller_id: &'static str) -> Router {
    let mut owners = HashMap::new();
    owners.insert("ws-alice".to_string(), "alice".to_string());
    owners.insert("ws-bob".to_string(), "bob".to_string());
    let resolver = TwoTenantResolver { state, owners };
    Router::new()
        .route("/v1/test", get(handler))
        .layer(WorkspaceRoutingLayer::new(resolver))
        .layer(from_fn(
            move |mut req: Request<Body>, next: axum::middleware::Next| async move {
                req.extensions_mut().insert(AuthPrincipal::user(caller_id));
                next.run(req).await
            },
        ))
}

async fn get_ws(app: Router, ws: &str) -> StatusCode {
    app.oneshot(
        Request::get("/v1/test")
            .header(WORKSPACE_HEADER, ws)
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
    .status()
}

#[tokio::test]
async fn owner_resolves_own_workspace_200() {
    let state = Arc::new(support::TestHarness::raw_state().await);
    let status = get_ws(app_for(state, "alice"), "ws-alice").await;
    assert_eq!(status, StatusCode::OK, "owner must reach the handler");
}

#[tokio::test]
async fn non_owner_is_forbidden_403() {
    let state = Arc::new(support::TestHarness::raw_state().await);
    let status = get_ws(app_for(state, "alice"), "ws-bob").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "caller acting on another tenant's workspace must be 403"
    );
}

#[tokio::test]
async fn unknown_workspace_is_404() {
    let state = Arc::new(support::TestHarness::raw_state().await);
    let status = get_ws(app_for(state, "alice"), "ws-nobody").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
