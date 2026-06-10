//! Plain session persistence across a full server restart (NOT
//! crash-recovery).
//!
//! Proves a session + its streamed assistant message, created against an
//! on-disk `OPENLET_DATA_DIR`, are still present after the server process
//! is dropped and a fresh one boots on the SAME dir. Storage is the real
//! `SqliteMemoryStore` + on-disk sqlite — no mocks of the storage layer.
//!
//! Crash-recovery (Running→Errored reconciliation) is deliberately NOT
//! exercised: that path lives only in `run_server` (the binary's boot),
//! which this harness bypasses by serving via `axum::serve` directly. This
//! test asserts plain persistence only.

use std::time::Duration;

use openlet_core::types::message::Role;
use openlet_core::types::part::Part;
use openlet_core::types::session::SessionId;
use openlet_test_mock_provider::{MockOpenAiService, SCENARIO_PREFIX};
use serde_json::Value;
use tempfile::TempDir;

mod live_support;
use live_support::LiveServer;

fn kinds(frames: &[Value]) -> Vec<String> {
    frames
        .iter()
        .filter_map(|f| f.get("kind").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

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

/// Boot#1 (on-disk) → create session + run a turn so a message persists →
/// drop the server → boot#2 on the SAME dir → assert the session is listed
/// and its assistant text replays from the durable sqlite event log.
#[tokio::test]
async fn session_and_messages_survive_a_full_restart() {
    // Caller-owned data dir so it survives the boot→drop→reboot cycle. The
    // mock is owned separately and stays up across both boots.
    let data_dir = TempDir::new().expect("data dir");
    let mock = MockOpenAiService::spawn().await.expect("mock");

    // ── Boot #1: create a session and stream one turn to disk ──────────
    let session_id;
    {
        let srv =
            LiveServer::with_mock_on_disk(mock.base_url(), data_dir.path().to_path_buf()).await;
        session_id = srv.create_session().await;

        srv.prompt(
            &session_id,
            &format!("{SCENARIO_PREFIX}simple_text persist me"),
        )
        .await;
        let frames = srv
            .collect_session_events(&session_id, Duration::from_secs(20))
            .await;
        assert!(
            kinds(&frames).iter().any(|k| k == "session_status"),
            "first turn must reach terminal status before we drop the server"
        );
        let streamed = assembled_text(&frames);
        assert!(
            streamed.contains("Hello"),
            "boot#1 should have streamed the scenario text; got {streamed:?}"
        );
        // srv drops here → serve task aborted, but the on-disk sqlite under
        // data_dir persists (open_pool, not open_in_memory).
    }

    // ── Boot #2: fresh server, SAME on-disk data dir ───────────────────
    let srv2 = LiveServer::with_mock_on_disk(mock.base_url(), data_dir.path().to_path_buf()).await;

    // The session created in boot#1 is listed by the fresh server.
    let sessions = srv2.get_json("/v1/session").await;
    let ids: Vec<&str> = sessions
        .as_array()
        .expect("session list array")
        .iter()
        .filter_map(|s| s.get("id").and_then(Value::as_str))
        .collect();
    assert!(
        ids.contains(&session_id.as_str()),
        "session {session_id} must persist across restart; saw {ids:?}"
    );

    // The assistant message + its text persisted. `part_delta` events are
    // Transient (never written to the durable event log), so the streamed
    // text is NOT recoverable via SSE replay — it lives in the parts table.
    // Read it back from boot#2's reopened on-disk sqlite via the real
    // SqliteMemoryStore (no storage mock): list messages, find the assistant
    // turn, and assert its persisted Text part survived the restart.
    let sid = SessionId::from(uuid::Uuid::parse_str(&session_id).expect("session uuid"));
    let memory = srv2.memory();
    let messages = memory.list_messages(sid).await.expect("list messages");
    assert!(
        !messages.is_empty(),
        "messages must persist across restart; got none for {session_id}"
    );
    let assistant = messages
        .iter()
        .find(|m| m.role == Role::Assistant)
        .expect("an assistant message must persist across the restart");

    let parts = memory
        .list_parts(sid, assistant.id)
        .await
        .expect("list parts");
    let persisted_text: String = parts
        .iter()
        .filter_map(|p| match p {
            Part::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        persisted_text.contains("Hello"),
        "assistant text must survive the restart in the parts table; got {persisted_text:?}"
    );
}
