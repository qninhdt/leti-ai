//! Workspace routing middleware.
//!
//! Resolves the `x-leti-workspace` header (header-only for MVP; path
//! prefix `/v1/workspaces/{id}/...` deferred) into an [`Arc<AppState>`]
//! via a [`WorkspaceResolver`] and stashes it on the request extensions
//! so per-route handlers can pull it out.
//!
//! ## Cross-tenant isolation gate
//!
//! The middleware asserts an [`AuthPrincipal`] is present in request
//! extensions BEFORE consulting the resolver. Without authentication, a
//! malicious caller could forge the `x-leti-workspace` header and read
//! another tenant's data — the middleware refuses to resolve a workspace
//! until auth has run. Mount order MUST be: auth middleware →
//! workspace_routing middleware → handler. Violating this order produces
//! 401 on every request, which is loud-fail (intentional).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use tower::{Layer, Service};

use crate::auth::AuthPrincipal;
use crate::workspace_resolver::{WorkspaceError, WorkspaceResolver};

/// HTTP header used to select the workspace for a request. Cloud
/// deployments require this on every authenticated request; the local
/// binary tolerates absence (resolver is single-tenant).
pub const WORKSPACE_HEADER: &str = "x-leti-workspace";

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
        // Gate: assert auth ran before resolver lookup. Absence
        // of AuthPrincipal in extensions → 401, no resolver call.
        let Some(principal) = req.extensions().get::<AuthPrincipal>().cloned() else {
            return Box::pin(async move {
                Ok((StatusCode::UNAUTHORIZED, "authentication required").into_response())
            });
        };

        let resolver = self.resolver.clone();
        let mut inner = self.inner.clone();
        let workspace_id = req
            .headers()
            .get(WORKSPACE_HEADER)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("default")
            .to_string();

        Box::pin(async move {
            match resolver.resolve(&principal, &workspace_id).await {
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
                Err(WorkspaceError::Forbidden(msg)) => {
                    Ok((StatusCode::FORBIDDEN, msg).into_response())
                }
                Err(WorkspaceError::LookupFailed(msg)) => {
                    Ok((StatusCode::SERVICE_UNAVAILABLE, msg).into_response())
                }
            }
        })
    }
}
