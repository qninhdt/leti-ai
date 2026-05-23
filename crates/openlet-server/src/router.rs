//! Axum router composition + Swagger UI mount.

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

/// Build the full router. The OpenApi doc is split out: utoipa-axum
/// produces a `Router` + an aggregated `OpenApi`; we attach Swagger UI to
/// the latter and merge.
pub fn build(state: AppState) -> Router {
    let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(health::handler))
        .routes(routes!(session::create, session::list))
        .routes(routes!(session::get_one, session::delete))
        .routes(routes!(session::set_mode))
        .routes(routes!(message::prompt_async))
        .routes(routes!(cancel::abort))
        .routes(routes!(agent::list))
        .routes(routes!(permission::reply))
        .routes(routes!(event::stream))
        .routes(routes!(plugin::list))
        .routes(routes!(plugin::health))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .split_for_parts();

    router
        .merge(SwaggerUi::new("/doc").url("/doc/openapi.json", api))
        .with_state(state)
}
