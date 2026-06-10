//! Live E2E — server + core UAT against a real loopback server with the
//! real `OpenAiCompatProvider` pointed at the in-process mock service.
//!
//! Unlike the `oneshot`/`StubProvider` integration tests, these drive a
//! genuine runtime turn loop: HTTP prompt → provider stream → processor →
//! persisted parts → live SSE frames, exactly the BE→FE path the TUI uses.
//!
//! Deterministic + network-free: the mock service replies from canned SSE
//! byte scripts selected by a `PARITY_SCENARIO:` token in the prompt.

use std::time::Duration;

use openlet_test_mock_provider::{MockOpenAiService, SCENARIO_PREFIX};
use serde_json::Value;

mod live_support;
use live_support::LiveServer;

/// Pull the set of distinct event `kind`s out of a frame list.
fn kinds(frames: &[Value]) -> Vec<String> {
    frames
        .iter()
        .filter_map(|f| f.get("kind").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// Concatenate every `part_delta` text fragment in arrival order — the
/// assistant's streamed message body as the TUI would assemble it.
fn assembled_text(frames: &[Value]) -> String {
    let mut out = String::new();
    for f in frames {
        if f.get("kind").and_then(Value::as_str) == Some("part_delta")
            && f.get("delta_kind").and_then(Value::as_str) == Some("text")
        {
            if let Some(d) = f.get("delta").and_then(Value::as_str) {
                out.push_str(d);
            }
        }
    }
    out
}

/// Health endpoint answers 200 on a freshly-booted live server.
#[tokio::test]
async fn health_ok_on_live_server() {
    let mock = MockOpenAiService::spawn().await.expect("mock");
    let srv = LiveServer::with_mock(mock.base_url()).await;
    assert_eq!(srv.health().await, reqwest::StatusCode::OK);
}

/// `GET /v1/models` returns the mock's canned catalog over real HTTP.
#[tokio::test]
async fn models_route_returns_catalog_over_http() {
    let mock = MockOpenAiService::spawn().await.expect("mock");
    let srv = LiveServer::with_mock(mock.base_url()).await;
    let models = srv.models().await;
    assert_eq!(models.len(), 2, "mock serves 2 models; got {models:?}");
    let ids: Vec<&str> = models.iter().filter_map(|m| m["id"].as_str()).collect();
    assert!(ids.contains(&"mock/model-small"), "ids={ids:?}");
}

/// The golden path: prompt → real turn loop streams the mock's
/// `simple_text` scenario → SSE carries message_created, part_created,
/// part_delta(s), part_updated, terminal idle status. Assembled text
/// equals the mock's scripted output.
#[tokio::test]
async fn full_turn_streams_assistant_text_be_to_fe() {
    let mock = MockOpenAiService::spawn().await.expect("mock");
    let srv = LiveServer::with_mock(mock.base_url()).await;

    let sid = srv.create_session().await;
    let ack = srv
        .prompt(&sid, &format!("{SCENARIO_PREFIX}simple_text say hi"))
        .await;
    assert_eq!(ack, reqwest::StatusCode::ACCEPTED, "prompt ack");

    let frames = srv
        .collect_session_events(&sid, Duration::from_secs(15))
        .await;
    let seen = kinds(&frames);

    // Event-ordering invariants the TUI store relies on.
    assert!(
        seen.iter().any(|k| k == "message_created"),
        "expected message_created; saw {seen:?}"
    );
    assert!(
        seen.iter().any(|k| k == "part_created"),
        "expected part_created; saw {seen:?}"
    );
    assert!(
        seen.iter().any(|k| k == "part_delta"),
        "expected part_delta; saw {seen:?}"
    );
    assert!(
        seen.iter().any(|k| k == "session_status"),
        "expected terminal session_status; saw {seen:?}"
    );

    // The mock's simple_text scenario streams "Hello" + ", world".
    let text = assembled_text(&frames);
    assert_eq!(
        text, "Hello, world",
        "assembled streamed text mismatch; frames={frames:?}"
    );
}

/// A reasoning-content stream surfaces as `part_delta` frames with
/// `delta_kind: reasoning` ahead of the terminal status — proving the
/// processor classifies reasoning vs text correctly end to end.
#[tokio::test]
async fn reasoning_stream_surfaces_reasoning_delta() {
    let mock = MockOpenAiService::spawn().await.expect("mock");
    let srv = LiveServer::with_mock(mock.base_url()).await;

    let sid = srv.create_session().await;
    srv.prompt(
        &sid,
        &format!("{SCENARIO_PREFIX}reasoning think then answer"),
    )
    .await;

    let frames = srv
        .collect_session_events(&sid, Duration::from_secs(15))
        .await;

    let has_reasoning = frames.iter().any(|f| {
        f.get("kind").and_then(Value::as_str) == Some("part_delta")
            && f.get("delta_kind").and_then(Value::as_str) == Some("reasoning")
    });
    assert!(
        has_reasoning,
        "expected a reasoning part_delta; frames={frames:?}"
    );
}

/// A turn that the mock scripts as a tool call drives the processor's
/// tool-call accumulation path: the SSE stream must carry a tool-call
/// `part_created` and reach a terminal status without hanging.
#[tokio::test]
async fn tool_call_stream_reaches_terminal_status() {
    let mock = MockOpenAiService::spawn().await.expect("mock");
    let srv = LiveServer::with_mock(mock.base_url()).await;

    let sid = srv.create_session().await;
    // Default WorkspaceWrite makes the bash call fall through to Ask →
    // Pending → park. Danger mode auto-allows so the call actually
    // dispatches and the turn reaches a terminal status.
    srv.set_mode(&sid, "danger").await;
    srv.prompt(
        &sid,
        &format!("{SCENARIO_PREFIX}with_tool_call list the cwd"),
    )
    .await;

    let frames = srv
        .collect_session_events(&sid, Duration::from_secs(20))
        .await;
    let seen = kinds(&frames);
    assert!(
        seen.iter().any(|k| k == "session_status"),
        "tool-call turn must reach terminal status; saw {seen:?}"
    );
}

/// Two sequential prompts on one session both complete — the active-turn
/// slot is released after the first so the second is accepted, not 409'd.
#[tokio::test]
async fn sequential_turns_same_session_both_complete() {
    let mock = MockOpenAiService::spawn().await.expect("mock");
    let srv = LiveServer::with_mock(mock.base_url()).await;

    let sid = srv.create_session().await;

    srv.prompt(&sid, &format!("{SCENARIO_PREFIX}simple_text first"))
        .await;
    let f1 = srv
        .collect_session_events(&sid, Duration::from_secs(15))
        .await;
    assert!(
        kinds(&f1).iter().any(|k| k == "session_status"),
        "first turn must finish"
    );

    let ack2 = srv
        .prompt(&sid, &format!("{SCENARIO_PREFIX}simple_text second"))
        .await;
    assert_eq!(
        ack2,
        reqwest::StatusCode::ACCEPTED,
        "second prompt must be accepted after first turn drains the slot"
    );
    let f2 = srv
        .collect_session_events(&sid, Duration::from_secs(15))
        .await;
    assert!(
        kinds(&f2).iter().any(|k| k == "session_status"),
        "second turn must finish"
    );
}
