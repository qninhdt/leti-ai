//! `AuthLayer` — runs the [`Authenticator`] on every request and injects
//! the resulting [`AuthPrincipal`] into request extensions.
//!
//! Mounted OUTSIDE the workspace-routing layer (auth runs first): the
//! workspace gate looks the `AuthPrincipal` up by `TypeId`, so auth must
//! have inserted it before the workspace service runs. On `AuthError` the
//! layer short-circuits with `401` and never calls the inner service.
//!
//! Hand-written Layer/Service (not `from_fn`) to mirror the sibling
//! [`WorkspaceRoutingLayer`] and keep the `Arc<dyn Authenticator>` field
//! Clone without forcing `Authenticator: Clone`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use tower::{Layer, Service};

use super::authenticator::Authenticator;

/// Tower layer that authenticates every request before the inner service.
pub struct AuthLayer {
    authenticator: Arc<dyn Authenticator>,
}

// Manual Clone — the `Arc<dyn _>` is always Clone regardless of the
// trait object behind it; `derive(Clone)` would over-constrain.
impl Clone for AuthLayer {
    fn clone(&self) -> Self {
        Self {
            authenticator: self.authenticator.clone(),
        }
    }
}

impl AuthLayer {
    #[must_use]
    pub fn new(authenticator: Arc<dyn Authenticator>) -> Self {
        Self { authenticator }
    }
}

impl<S> Layer<S> for AuthLayer {
    type Service = AuthService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthService {
            authenticator: self.authenticator.clone(),
            inner,
        }
    }
}

pub struct AuthService<S> {
    authenticator: Arc<dyn Authenticator>,
    inner: S,
}

impl<S: Clone> Clone for AuthService<S> {
    fn clone(&self) -> Self {
        Self {
            authenticator: self.authenticator.clone(),
            inner: self.inner.clone(),
        }
    }
}

impl<S> Service<Request<Body>> for AuthService<S>
where
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
        let authenticator = self.authenticator.clone();
        // Clone the ready inner per tower's contract: the `self.inner` we
        // hold may not be the one that returned Ready from poll_ready.
        let mut inner = self.inner.clone();
        Box::pin(async move {
            match authenticator.authenticate(req.headers()).await {
                Ok(principal) => {
                    req.extensions_mut().insert(principal);
                    inner.call(req).await
                }
                Err(e) => {
                    // Log the reason server-side; return an opaque 401 so
                    // we don't oracle token internals to the caller.
                    tracing::debug!(error = %e, "authentication rejected");
                    Ok((StatusCode::UNAUTHORIZED, "authentication required").into_response())
                }
            }
        })
    }
}
