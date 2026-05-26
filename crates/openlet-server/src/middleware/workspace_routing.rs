//! Workspace routing middleware + extractor.
//!
//! Resolves the `x-openlet-workspace` header (header-only for MVP; path
//! prefix `/v1/workspaces/{id}/...` deferred) into an [`Arc<AppState>`]
//! via a [`WorkspaceResolver`] and stashes it on the request extensions
//! so per-route handlers can pull it out via [`WorkspaceRoutingGuard`].
//!
//! ## Cross-tenant isolation gate (F5.1)
//!
//! [`WorkspaceRoutingGuard`] asserts an [`AuthPrincipal`] is present in
//! request extensions BEFORE consulting the resolver. Without
//! authentication, a malicious caller could forge the
//! `x-openlet-workspace` header and read another tenant's data —
//! the guard refuses to resolve a workspace until auth has run. Mount
//! order MUST be: auth middleware → workspace_routing middleware →
//! handler. Violating this order produces 401 on every request, which
//! is loud-fail (intentional).

use std::sync::Arc;

use axum::body::Body;
use axum::extract::FromRequestParts;
use axum::http::{Request, StatusCode, request::Parts};
use axum::response::{IntoResponse, Response};
use tower::{Layer, Service};

use crate::app_state::AppState;
use crate::workspace_resolver::{WorkspaceError, WorkspaceResolver};

/// HTTP header used to select the workspace for a request. Cloud
/// deployments require this on every authenticated request; the local
/// binary tolerates absence (resolver is single-tenant).
pub const WORKSPACE_HEADER: &str = "x-openlet-workspace";

/// Authenticated principal — populated by upstream auth middleware
/// (Phase 1's ask_user route uses an env-key fallback). The presence
/// of this extension is the F5.1 cross-tenant gate.
///
/// Cloud integrators replace this struct with their own (mounting
/// before workspace_routing); the type identity is what the guard
/// looks up. Re-exported here so non-server crates can construct it
/// in tests.
#[derive(Debug, Clone)]
pub struct AuthPrincipal {
    pub subject: String,
}

/// Extractor for handlers that need the resolved per-workspace state.
/// Returns 401 if auth hasn't run, 400 for malformed workspace ids,
/// 404 for unknown workspaces.
pub struct WorkspaceRoutingGuard {
    pub state: Arc<AppState>,
    pub principal: AuthPrincipal,
}

impl<S> FromRequestParts<S> for WorkspaceRoutingGuard
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let state = parts
            .extensions
            .get::<Arc<AppState>>()
            .cloned()
            .ok_or_else(|| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "workspace state not resolved",
                )
                    .into_response()
            })?;
        let principal = parts
            .extensions
            .get::<AuthPrincipal>()
            .cloned()
            .ok_or_else(|| (StatusCode::UNAUTHORIZED, "authentication required").into_response())?;
        Ok(WorkspaceRoutingGuard { state, principal })
    }
}

/// Layer that resolves the workspace header on every request and
/// inserts the [`Arc<AppState>`] into request extensions. Returns
/// 401 when no [`AuthPrincipal`] extension is present.
pub struct WorkspaceRoutingLayer<R> {
    resolver: Arc<R>,
}

// Manual Clone — `derive(Clone)` would require `R: Clone`, but the
// `Arc<R>` field is always Clone regardless of `R`.
impl<R> Clone for WorkspaceRoutingLayer<R> {
    fn clone(&self) -> Self {
        Self {
            resolver: self.resolver.clone(),
        }
    }
}

impl<R> WorkspaceRoutingLayer<R>
where
    R: WorkspaceResolver,
{
    #[must_use]
    pub fn new(resolver: R) -> Self {
        Self {
            resolver: Arc::new(resolver),
        }
    }
}

impl<R, S> Layer<S> for WorkspaceRoutingLayer<R>
where
    R: WorkspaceResolver,
{
    type Service = WorkspaceRoutingService<R, S>;

    fn layer(&self, inner: S) -> Self::Service {
        WorkspaceRoutingService {
            resolver: self.resolver.clone(),
            inner,
        }
    }
}

pub struct WorkspaceRoutingService<R, S> {
    resolver: Arc<R>,
    inner: S,
}

// Manual Clone — same reason as `WorkspaceRoutingLayer`: avoid an
// over-constrained `R: Clone` requirement so resolvers without their
// own Clone impl still work.
impl<R, S: Clone> Clone for WorkspaceRoutingService<R, S> {
    fn clone(&self) -> Self {
        Self {
            resolver: self.resolver.clone(),
            inner: self.inner.clone(),
        }
    }
}

impl<R, S> Service<Request<Body>> for WorkspaceRoutingService<R, S>
where
    R: WorkspaceResolver,
    S: Service<Request<Body>, Response = Response> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = S::Error;
    type Future =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<Response, S::Error>> + Send>>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<Body>) -> Self::Future {
        // F5.1 gate: assert auth ran before resolver lookup. Absence
        // of AuthPrincipal in extensions → 401, no resolver call.
        if req.extensions().get::<AuthPrincipal>().is_none() {
            return Box::pin(async move {
                Ok((StatusCode::UNAUTHORIZED, "authentication required").into_response())
            });
        }

        let resolver = self.resolver.clone();
        let mut inner = self.inner.clone();
        let workspace_id = req
            .headers()
            .get(WORKSPACE_HEADER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("default")
            .to_string();

        Box::pin(async move {
            match resolver.resolve(&workspace_id).await {
                Ok(state) => {
                    req.extensions_mut().insert(state);
                    inner.call(req).await
                }
                Err(WorkspaceError::Invalid(msg)) => {
                    Ok((StatusCode::BAD_REQUEST, msg).into_response())
                }
                Err(WorkspaceError::NotFound(msg)) => {
                    Ok((StatusCode::NOT_FOUND, msg).into_response())
                }
                Err(WorkspaceError::LookupFailed(msg)) => {
                    Ok((StatusCode::SERVICE_UNAVAILABLE, msg).into_response())
                }
            }
        })
    }
}
