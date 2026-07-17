//! `GET /v1/models` route wiring.
//!
//! The harness provider uses the trait's default `list_models` (empty
//! catalog), so this test asserts the route is mounted, returns 200, and
//! serializes a JSON array. The catalog-decode path against a populated
//! upstream is covered by `leti-adapters/tests/openai_compat_list_models.rs`.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use leti_protocol::ModelDto;
use tower::util::ServiceExt;

mod support;

#[tokio::test]
async fn get_models_returns_200_and_json_array() {
    let harness = support::TestHarness::new().await;
    let app = harness.router();

    let resp = app
        .oneshot(Request::get("/v1/models").body(Body::empty()).unwrap())
        .await
        .expect("models route");

    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let models: Vec<ModelDto> =
        serde_json::from_slice(&bytes).expect("body decodes as Vec<ModelDto>");
    // Default-impl provider advertises no catalog → empty array, not an error.
    assert!(
        models.is_empty(),
        "stub provider should yield []; got {models:?}"
    );
}
