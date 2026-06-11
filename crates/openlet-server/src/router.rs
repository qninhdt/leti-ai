//! Axum router composition + Swagger UI mount.
//!
//! [`RouterBuilder`] exposes per-feature subrouters so downstream
//! integrators can mount only what they need (or override individual
//! routes by skipping ours and merging their own). [`build`] is kept as a
//! thin wrapper for the reference binary + integration tests.

use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use utoipa_swagger_ui::SwaggerUi;

use crate::app_state::AppState;
use crate::auth::{AuthLayer, Authenticator, LocalDevAuthenticator};
use crate::middleware::WorkspaceRoutingLayer;
use crate::openapi::ApiDoc;
use crate::routes::{
    agent, attachments, cancel, diagnostics, event, files, health, message, model, permission,
    plugin, question, session,
};
use crate::workspace_resolver::StaticWorkspaceResolver;

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
            .with_question_routes()
            .with_agent_routes()
            .with_model_routes()
            .with_plugin_routes()
            .with_diagnostics_routes()
            .with_attachment_routes()
            .with_files_routes()
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

    /// `POST /v1/session/:id/prompt_async` + `GET /v1/session/:id/messages`.
    #[must_use]
    pub fn with_message_routes(mut self) -> Self {
        self.inner = self
            .inner
            .routes(routes!(message::prompt_async))
            .routes(routes!(message::list_messages));
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

    /// `POST /v1/sessions/:id/question/answer`.
    #[must_use]
    pub fn with_question_routes(mut self) -> Self {
        self.inner = self.inner.routes(routes!(question::answer));
        self
    }

    /// `GET /v1/agent`.
    #[must_use]
    pub fn with_agent_routes(mut self) -> Self {
        self.inner = self.inner.routes(routes!(agent::list));
        self
    }

    /// `GET /v1/models` — provider model catalog.
    #[must_use]
    pub fn with_model_routes(mut self) -> Self {
        self.inner = self.inner.routes(routes!(model::list));
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

    /// `GET /v1/diagnostics` — preflight report (api key, data dir, sqlite,
    /// plugins, model reachability, port). Output is redacted.
    #[must_use]
    pub fn with_diagnostics_routes(mut self) -> Self {
        self.inner = self.inner.routes(routes!(diagnostics::report));
        self
    }

    /// `POST /v1/sessions/:id/attachments` — multipart upload. Body is
    /// capped at 25MB via [`attachments::body_limit_layer`], a
    /// route-specific `RequestBodyLimitLayer` that disables the global
    /// 2MB cap.
    #[must_use]
    pub fn with_attachment_routes(mut self) -> Self {
        let layered = OpenApiRouter::with_openapi(ApiDoc::openapi())
            .routes(routes!(attachments::upload))
            .layer(attachments::body_limit_layer());
        self.inner = self.inner.merge(layered);
        self
    }

    /// `GET /v1/files` + `GET /v1/files/content` — workspace file listing +
    /// content for the TUI @-mention feature (mock data this phase).
    #[must_use]
    pub fn with_files_routes(mut self) -> Self {
        self.inner = self
            .inner
            .routes(routes!(files::list))
            .routes(routes!(files::content));
        self
    }

    /// Finalize with the local dev authenticator. Equivalent to
    /// `build_with_auth(state, LocalDevAuthenticator::default())`. This is
    /// the local-binary + integration-test entry point; cloud binaries
    /// call [`build_with_auth`](Self::build_with_auth) with their own
    /// verifier.
    pub fn build(self, state: AppState) -> Router {
        self.build_with_auth(state, Arc::new(LocalDevAuthenticator::default()))
    }

    /// Finalize: mount auth + workspace-routing, attach trace+cors layers,
    /// mount Swagger UI from the accumulated OpenAPI doc, bind the state.
    ///
    /// Layer order (outermost → innermost as a request descends):
    /// BodyLimit → CORS → Trace → `AuthLayer` → `WorkspaceRoutingLayer` →
    /// handler. CORS sits OUTSIDE auth so browser `OPTIONS` preflight is
    /// answered without a credential; the body limit caps the request
    /// before auth runs. Auth runs before workspace routing so the
    /// injected `AuthPrincipal` is present for the workspace gate.
    ///
    /// CORS defaults to a closed policy (no origins allowed). Set
    /// `OPENLET_CORS_ALLOW_ORIGINS` to a comma-separated origin list
    /// (e.g. `https://app.example.com,https://admin.example.com`) to
    /// allow cross-origin browsers; set `OPENLET_CORS_PERMISSIVE=1` for
    /// dev-only `Access-Control-Allow-Origin: *` (warns on boot).
    pub fn build_with_auth(self, state: AppState, authenticator: Arc<dyn Authenticator>) -> Router {
        // The workspace resolver stays single-tenant (Static) this phase;
        // the dynamic/cloud resolver lands in a later phase. It resolves
        // any well-formed id to the one shared state.
        let workspace_layer =
            WorkspaceRoutingLayer::new(StaticWorkspaceResolver::new(Arc::new(state.clone())));

        let (router, api) = self
            .inner
            // Innermost of the middleware stack: workspace routing, then
            // auth ABOVE it (auth runs first on the way in).
            .layer(workspace_layer)
            .layer(AuthLayer::new(authenticator))
            .layer(TraceLayer::new_for_http())
            .layer(build_cors_layer())
            // 2 MiB global body limit applies to ALL routes, not only
            // Json<T> extractors. Closes Reviewer C important finding —
            // any non-Json extractor (raw Bytes, future multipart) was
            // previously unbounded.
            .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
            .split_for_parts();

        // Gate Swagger UI on OPENLET_ENABLE_DOCS. Defaults to ON for
        // local-binary developer ergonomics, OFF for cloud-binary builds
        // where the docs surface is an unnecessary attack vector
        // (Swagger UI has had XSS history). Operators set
        // OPENLET_ENABLE_DOCS=0 in cloud deploys.
        let docs_enabled = std::env::var("OPENLET_ENABLE_DOCS")
            .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
            .unwrap_or(true);

        if docs_enabled {
            router
                .merge(SwaggerUi::new("/doc").url("/doc/openapi.json", api))
                .with_state(state)
        } else {
            router.with_state(state)
        }
    }
}

/// Resolves the CORS layer from env. Closed by default; opt-in to
/// per-origin allowlist or permissive mode.
fn build_cors_layer() -> CorsLayer {
    if std::env::var("OPENLET_CORS_PERMISSIVE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        tracing::warn!(
            "OPENLET_CORS_PERMISSIVE=1 — CORS layer accepts any origin; \
             do not enable in production"
        );
        return CorsLayer::permissive();
    }

    if let Ok(origins) = std::env::var("OPENLET_CORS_ALLOW_ORIGINS") {
        let parsed: Vec<_> = origins
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.parse::<axum::http::HeaderValue>().ok())
            .collect();
        if !parsed.is_empty() {
            return CorsLayer::new()
                .allow_origin(parsed)
                .allow_methods(tower_http::cors::AllowMethods::mirror_request())
                .allow_headers(tower_http::cors::AllowHeaders::mirror_request());
        }
    }

    CorsLayer::new()
}

/// Backward-compatible monolithic build. Equivalent to
/// `RouterBuilder::default().build(state)`. Kept so the reference binary
/// + existing integration tests don't churn.
pub fn build(state: AppState) -> Router {
    RouterBuilder::default().build(state)
}
