//! Zero-mock FS-write E2E (CI tier, deterministic, default-run).
//!
//! Proves a real model turn → real dispatch → real `LocalFilesystem` →
//! on-disk state, with ZERO mocks of the fs/storage/permission layer. Only
//! the LLM is the in-process mock, and even that emits a genuine
//! OpenAI-compat tool-call frame the real turn loop executes.
//!
//! The mock is stateless, so it cannot script a multi-step
//! create→read→edit→delete sequence (turn 2 would re-detect the same token
//! and replay the write forever). This tier is therefore scoped to ONE
//! `write` call; the full CRUD sequence is the gated real-OpenRouter tier's
//! job (`live_e2e_fs_agent_crud.rs`), where a real model advances the steps.

use std::time::Duration;

use leti_test_mock_provider::{
    FS_WRITE_CONTENT, FS_WRITE_PATH, MockOpenAiService, SCENARIO_PREFIX,
};
use serde_json::Value;

mod live_support;
use live_support::LiveServer;

fn kinds(frames: &[Value]) -> Vec<String> {
    frames
        .iter()
        .filter_map(|f| f.get("kind").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// A single `write` tool-call, driven by a real turn loop against the
/// deterministic mock LLM, lands a file on real disk through the real
/// permission gate (no fs/storage/permission mocks).
#[tokio::test]
async fn fs_write_once_hits_real_disk_through_real_gate() {
    let mock = MockOpenAiService::spawn().await.expect("mock");
    let srv = LiveServer::with_mock(mock.base_url()).await;

    let sid = srv.create_session().await;
    // Default WorkspaceWrite makes `write` fall through to Ask → Pending →
    // park (the exact Phase-1 silent-hang). Danger mode permits all, so the
    // call dispatches against the REAL ConfigPermissionMgr (not a mock gate)
    // and runs to completion.
    let mode = srv.set_mode(&sid, "danger").await;
    assert_eq!(mode, reqwest::StatusCode::OK, "set danger mode");

    srv.prompt(&sid, &format!("{SCENARIO_PREFIX}fs_write_once please"))
        .await;

    let frames = srv
        .collect_session_events(&sid, Duration::from_secs(20))
        .await;
    let seen = kinds(&frames);
    assert!(
        seen.iter().any(|k| k == "session_status"),
        "write turn must reach a terminal status; saw {seen:?}"
    );

    // The real LocalFilesystem wrote the file under the agent workspace.
    let path = srv.workspace_root().join(FS_WRITE_PATH);
    let on_disk = tokio::fs::read_to_string(&path)
        .await
        .unwrap_or_else(|e| panic!("scenario file {} must exist on disk: {e}", path.display()));
    assert_eq!(
        on_disk, FS_WRITE_CONTENT,
        "file contents must match the scripted write"
    );
}
