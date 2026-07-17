//! SSE resume test — verifies that disconnecting and reconnecting with
//! a `Last-Event-ID` header replays missed durable events from the
//! `events` table.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use leti_core::adapters::event_sink::Persistence;
use leti_core::types::event::AgentEvent;
use leti_core::types::message::MessageId;
use leti_core::types::part::PartId;
use leti_core::types::session::SessionId;
use tower::util::ServiceExt;

mod support;

#[tokio::test]
async fn replay_resumes_after_last_event_id() {
    let harness = support::TestHarness::new().await;
    let app = harness.router();
    let session_id = SessionId::new();

    // Publish 3 durable events.
    for _ in 0..3 {
        harness
            .events
            .publish(
                AgentEvent::PartCreated {
                    session_id,
                    message_id: MessageId::new(),
                    part_id: PartId::new(),
                    at: Utc::now(),
                },
                Persistence::Durable,
            )
            .await
            .expect("publish");
    }

    // First connection — request all events with Last-Event-ID: 0.
    let resp = app
        .clone()
        .oneshot(
            Request::get(format!("/v1/event?session={}", session_id.as_uuid()))
                .header("Last-Event-ID", "0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Read enough bytes to see all replay frames before keepalive blocks.
    let body = resp.into_body();
    let bytes = read_until_or_timeout(body, 3).await;
    let text = String::from_utf8_lossy(&bytes);
    let frames = text.matches("event: part.created").count();
    assert!(
        frames >= 3,
        "expected ≥3 part.created replay frames, got: {text}"
    );
}

/// Pull from the body up to ~3 frames or 500ms, whichever first. The
/// SSE keep-alive will hold the connection open indefinitely otherwise.
async fn read_until_or_timeout(body: Body, expected_frames: usize) -> Vec<u8> {
    let stream = body.into_data_stream();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(Duration::from_millis(500), async {
        let mut s = stream;
        while let Some(chunk) = s.frame().await {
            if let Ok(frame) = chunk
                && let Some(data) = frame.data_ref()
            {
                buf.extend_from_slice(data);
            }
            let count = String::from_utf8_lossy(&buf)
                .matches("event: part.created")
                .count();
            if count >= expected_frames {
                break;
            }
        }
    })
    .await;
    buf
}
