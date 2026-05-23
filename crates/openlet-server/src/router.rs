//! Axum router composition + Swagger UI mount.

use axum::Router;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use utoipa_swagger_ui::SwaggerUi;

use crate::app_state::AppState;
use crate::openapi::ApiDoc;
use crate::routes::health;

/// Build the full router. The OpenApi doc is split out: utoipa-axum
/// produces a `Router` + an aggregated `OpenApi`; we attach Swagger UI to
/// the latter and merge.
pub fn build(state: AppState) -> Router {
    let (router, api) = OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(health::handler))
        .split_for_parts();

    router
        .merge(SwaggerUi::new("/doc").url("/doc/openapi.json", api))
        .with_state(state)
}
