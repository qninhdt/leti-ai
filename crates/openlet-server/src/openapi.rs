//! utoipa OpenAPI aggregator.
//!
//! Each route module exposes `#[utoipa::path]`-annotated handlers; the
//! `OpenApi` derive below collects them into the `/doc/openapi.json` doc.

use openlet_protocol::HealthDto;
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Openlet Agent Core API",
        version = "0.1.0",
        description = "REST + SSE surface for the Openlet agent runtime.",
        license(name = "Apache-2.0")
    ),
    paths(crate::routes::health::handler),
    components(schemas(HealthDto)),
    tags(
        (name = "global", description = "Server-wide endpoints (health, version)")
    )
)]
pub struct ApiDoc;
