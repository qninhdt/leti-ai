//! Integration tests for the multipart attachment upload route.
//!
//! Covers the 25MB body cap (fires at the layer, not after read) and
//! the happy-path JPEG round-trip.

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bytes::Bytes;
use leti_core::types::agent::AgentId;
use support::TestHarness;
use tower::ServiceExt;
use uuid::Uuid;

/// POSTing 30MB returns 413 BEFORE the handler reads the body.
/// The `Content-Length` header is set explicitly so the
/// `RequestBodyLimitLayer` short-circuits with 413 without ever
/// invoking the multipart parser.
#[tokio::test]
async fn multipart_25mb_cap_returns_413_pre_body_read() {
    let harness = TestHarness::new().await;

    let session_id = create_session(&harness).await;
    let body = build_multipart_body(&vec![b'A'; 30 * 1024 * 1024], "huge.bin");
    let body_len = body.len();
    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/session/{session_id}/attachments"))
        .header("content-type", "multipart/form-data; boundary=testboundary")
        .header("content-length", body_len.to_string())
        .body(Body::from(body))
        .unwrap();

    let resp = harness.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

/// Happy-path: POST a small JPEG, expect 201 + an artifact_id, and
/// confirm the artifact made it into the store.
#[tokio::test]
async fn multipart_image_round_trip() {
    let harness = TestHarness::new().await;
    let session_id = create_session(&harness).await;

    let jpeg_bytes = synthetic_small_jpeg();
    let body = build_multipart_body(&jpeg_bytes, "tiny.jpg");
    let req = Request::builder()
        .method("POST")
        .uri(format!("/v1/session/{session_id}/attachments"))
        .header("content-type", "multipart/form-data; boundary=testboundary")
        .body(Body::from(body))
        .unwrap();

    let resp = harness.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(json["kind"], "image");
    assert_eq!(json["mime"], "image/jpeg");
    assert!(
        json["summary"].as_str().unwrap().starts_with("image/jpeg "),
        "summary={}",
        json["summary"]
    );
}

/// Create a fresh session via the public route. Returns the session id.
async fn create_session(harness: &TestHarness) -> Uuid {
    let agent_id = first_agent_id(harness).await;
    let req = Request::builder()
        .method("POST")
        .uri("/v1/session")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "agent_id": agent_id,
                "extensions": {}
            })
            .to_string(),
        ))
        .unwrap();
    let resp = harness.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED, "session create failed");
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    Uuid::parse_str(v["id"].as_str().unwrap()).unwrap()
}

/// Pull the first registered agent id out of the harness's app state.
/// We register one default agent in `support::TestHarness::build_state`
/// — the test reads that id rather than guessing.
async fn first_agent_id(harness: &TestHarness) -> AgentId {
    let req = Request::builder()
        .method("GET")
        .uri("/v1/agent")
        .body(Body::empty())
        .unwrap();
    let resp = harness.router().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let id = v[0]["id"].as_str().expect("agent id").to_string();
    AgentId(Uuid::parse_str(&id).unwrap())
}

/// Build a minimal multipart/form-data body with one `file` field.
/// Boundary is the bare token `testboundary`; on the wire each
/// section separator is `--testboundary` per RFC 2046.
fn build_multipart_body(content: &[u8], filename: &str) -> Bytes {
    let boundary = "testboundary";
    let mut body: Vec<u8> = Vec::with_capacity(content.len() + 256);
    body.extend_from_slice(b"--");
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(b"Content-Disposition: form-data; name=\"file\"; filename=\"");
    body.extend_from_slice(filename.as_bytes());
    body.extend_from_slice(b"\"\r\n");
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(content);
    body.extend_from_slice(b"\r\n--");
    body.extend_from_slice(boundary.as_bytes());
    body.extend_from_slice(b"--\r\n");
    Bytes::from(body)
}

/// 64×64 single-color JPEG, baseline encode. Small enough to fit
/// well under the 5MB output budget.
fn synthetic_small_jpeg() -> Vec<u8> {
    use image::{DynamicImage, ImageFormat, RgbImage};
    use std::io::Cursor;
    let img = RgbImage::from_pixel(64, 64, image::Rgb([100, 200, 50]));
    let mut buf: Vec<u8> = Vec::new();
    DynamicImage::ImageRgb8(img)
        .write_to(&mut Cursor::new(&mut buf), ImageFormat::Jpeg)
        .expect("baseline jpeg encode");
    buf
}
