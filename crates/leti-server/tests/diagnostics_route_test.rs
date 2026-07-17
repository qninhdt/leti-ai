//! Integration test for `GET /v1/diagnostics`. Boots the full default
//! router via the test harness and verifies the route is mounted, the
//! response is well-formed JSON with the expected shape, and that no
//! token-shaped values leak through the redactor.

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::Value;
use tower::util::ServiceExt;

mod support;

#[tokio::test]
async fn diagnostics_route_returns_redacted_report() {
    let harness = support::TestHarness::new().await;
    let app = harness.router();

    let resp = app
        .oneshot(Request::get("/v1/diagnostics").body(Body::empty()).unwrap())
        .await
        .expect("send /v1/diagnostics");
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).expect("response is JSON");

    // Shape: {checks: [...], overall: "..."}.
    let checks = body.get("checks").and_then(Value::as_array).unwrap();
    assert!(
        !checks.is_empty(),
        "expected at least one check, got {body}"
    );
    let overall = body.get("overall").and_then(Value::as_str).unwrap();
    assert!(
        ["healthy", "degraded", "failed"].contains(&overall),
        "unexpected overall: {overall}"
    );

    // Every check has the required fields.
    for c in checks {
        assert!(c.get("name").is_some(), "check missing name: {c}");
        assert!(c.get("status").is_some(), "check missing status: {c}");
        assert!(
            c.get("elapsed_ms").is_some(),
            "check missing elapsed_ms: {c}"
        );
    }

    // Defense-in-depth: no token-shaped string should ever appear in the
    // response, even if a future check accidentally serializes one. The
    // redactor catches `sk-…` patterns of 16+ chars.
    let dumped = body.to_string();
    let needle = "sk-thisisexactlytwentychars";
    assert!(!dumped.contains(needle), "redactor must scrub: {dumped}");
}

#[tokio::test]
async fn diagnostics_includes_expected_check_names() {
    let harness = support::TestHarness::new().await;
    let app = harness.router();

    let resp = app
        .oneshot(Request::get("/v1/diagnostics").body(Body::empty()).unwrap())
        .await
        .expect("send /v1/diagnostics");
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let names: Vec<String> = body
        .get("checks")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .filter_map(|c| c.get("name").and_then(Value::as_str).map(str::to_owned))
        .collect();

    for expected in [
        "api_key_set",
        "data_dir_writable",
        "sqlite_health",
        "plugin_lifecycle",
        "model_reachable",
        "port_free",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "missing check `{expected}`, got {names:?}"
        );
    }
}
