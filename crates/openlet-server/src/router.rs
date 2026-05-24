//! Axum router composition + Swagger UI mount.
//!
//! [`RouterBuilder`] exposes per-feature subrouters so downstream
//! integrators can mount only what they need (or override individual
//! routes by skipping ours and merging their own). [`build`] is kept as a
//! thin wrapper for the reference binary + integration tests.

use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use utoipa_swagger_ui::SwaggerUi;

use crate::app_state::AppState;
use crate::openapi::ApiDoc;
use crate::routes::{agent, cancel, event, health, message, permission, plugin, session};

/// Fluent router composer. Call `with_*_routes()` to attach a feature
/// group; call `build(state)` to finalize into an `axum::Router`.
///
/// `RouterBuilder::default()` mounts every route group — matches today's
/// monolithic `build_router` behavior so the local binary keeps booting
/// unchanged.
pub struct RouterBuilder {
    inner: OpenApiRouter<AppState>,
}

impl Default for RouterBuilder {
    fn default() -> Self {
        Self::new()
            .with_health_routes()
            .with_session_routes()
            .with_message_routes()
            .with_event_routes()
            .with_permission_routes()
            .with_agent_routes()
            .with_plugin_routes()
    }
}

impl RouterBuilder {
    /// Empty router (no route groups attached). Use this when you want to
    /// pick a subset of `with_*_routes` rather than everything.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: OpenApiRouter::with_openapi(ApiDoc::openapi()),
        }
    }

    /// `GET /v1/health` — readiness probe.
    #[must_use]
    pub fn with_health_routes(mut self) -> Self {
        self.inner = self.inner.routes(routes!(health::handler));
        self
    }

    /// `POST/GET/DELETE /v1/session*` + `POST /v1/session/:id/abort`.
    #[must_use]
    pub fn with_session_routes(mut self) -> Self {
        self.inner = self
            .inner
            .routes(routes!(session::create, session::list))
            .routes(routes!(session::get_one, session::delete))
            .routes(routes!(session::set_mode))
            .routes(routes!(cancel::abort));
        self
    }

    /// `POST /v1/session/:id/message`.
    #[must_use]
    pub fn with_message_routes(mut self) -> Self {
        self.inner = self.inner.routes(routes!(message::prompt_async));
        self
    }

    /// `GET /v1/session/:id/events` (SSE stream).
    #[must_use]
    pub fn with_event_routes(mut self) -> Self {
        self.inner = self.inner.routes(routes!(event::stream));
        self
    }

    /// `POST /v1/permission/:ask_id`.
    #[must_use]
    pub fn with_permission_routes(mut self) -> Self {
        self.inner = self.inner.routes(routes!(permission::reply));
        self
    }

    /// `GET /v1/agent`.
    #[must_use]
    pub fn with_agent_routes(mut self) -> Self {
        self.inner = self.inner.routes(routes!(agent::list));
        self
    }

    /// `GET /v1/plugin` + `GET /v1/plugin/:id/health`.
    #[must_use]
    pub fn with_plugin_routes(mut self) -> Self {
        self.inner = self
            .inner
            .routes(routes!(plugin::list))
            .routes(routes!(plugin::health));
        self
    }

    /// Finalize: attach trace+cors layers, mount Swagger UI from the
    /// accumulated OpenAPI doc, bind the state.
    pub fn build(self, state: AppState) -> Router {
        let (router, api) = self
            .inner
            .layer(TraceLayer::new_for_http())
            .layer(CorsLayer::permissive())
            .split_for_parts();

        router
            .merge(SwaggerUi::new("/doc").url("/doc/openapi.json", api))
            .with_state(state)
    }
}

/// Backward-compatible monolithic build. Equivalent to
/// `RouterBuilder::default().build(state)`. Kept so the reference binary
/// + existing integration tests don't churn.
pub fn build(state: AppState) -> Router {
    RouterBuilder::default().build(state)
}
