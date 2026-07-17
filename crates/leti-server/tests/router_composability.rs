//! Verifies the `RouterBuilder` per-group composition. The default
//! builder mounts every route; selective builders return 404 on routes
//! that weren't wired in. Local binary's behavior (always-default) is
//! covered by other integration tests; this one nails the seam.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use leti_server::RouterBuilder;
use serde_json::json;
use tower::util::ServiceExt;

mod support;

#[tokio::test]
async fn default_builder_mounts_every_group() {
    let harness = support::TestHarness::new().await;
    let app = harness.router();

    let resp = app
        .clone()
        .oneshot(Request::get("/v1/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "health on default builder");

    let resp = app
        .clone()
        .oneshot(Request::get("/v1/agent").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "agent on default builder");

    let resp = app
        .clone()
        .oneshot(Request::get("/v1/plugin").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "plugin on default builder");
}

#[tokio::test]
async fn subset_builder_omits_unmounted_routes() {
    let state = support::TestHarness::raw_state().await;
    let app = RouterBuilder::new()
        .with_health_routes()
        .with_session_routes()
        .build(state);

    // Mounted: health works.
    let resp = app
        .clone()
        .oneshot(Request::get("/v1/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Mounted: session list works.
    let resp = app
        .clone()
        .oneshot(Request::get("/v1/session").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // NOT mounted: agent route 404s.
    let resp = app
        .clone()
        .oneshot(Request::get("/v1/agent").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND, "agent must be absent");

    // NOT mounted: plugin route 404s.
    let resp = app
        .clone()
        .oneshot(Request::get("/v1/plugin").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "plugin must be absent"
    );
}

#[tokio::test]
async fn empty_builder_serves_only_doc_endpoints() {
    let state = support::TestHarness::raw_state().await;
    let app = RouterBuilder::new().build(state);

    let resp = app
        .clone()
        .oneshot(Request::get("/v1/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "no groups → no /v1/* routes"
    );

    let resp = app
        .oneshot(Request::get("/v1/session").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// Locks the seam the cloud binary actually uses: integrator skips
/// `with_session_routes`, mounts a custom `POST /v1/session` upstream,
/// and merges the core router. The custom handler must win.
#[tokio::test]
async fn integrator_override_wins_on_shared_path() {
    async fn stub_create() -> Json<serde_json::Value> {
        Json(json!({"overridden": true}))
    }

    let state = support::TestHarness::raw_state().await;
    let core_router = RouterBuilder::new()
        .with_health_routes()
        .with_message_routes()
        .with_event_routes()
        .with_permission_routes()
        .with_agent_routes()
        .with_plugin_routes()
        .build(state);

    let app = Router::new()
        .route("/v1/session", post(stub_create))
        .merge(core_router);

    // Custom POST /v1/session wins (returns the stub body).
    let resp = app
        .clone()
        .oneshot(
            Request::post("/v1/session")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "override must respond 200");
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 16)
        .await
        .unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body, json!({"overridden": true}));

    // Core's other route groups still respond from `core_router`.
    let resp = app
        .oneshot(Request::get("/v1/health").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "health still mounted");
}
