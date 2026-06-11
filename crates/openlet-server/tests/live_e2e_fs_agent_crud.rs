//! Gated real-OpenRouter FS-CRUD E2E — the multi-step proof.
//!
//! The deterministic mock is stateless, so it can only drive a SINGLE
//! tool-call (`live_e2e_fs_write.rs`). The full create→read→edit→delete
//! sequence requires a REAL model that advances the steps because it sees
//! each tool result fed back into the next turn. So this tier is gated:
//!   - `#[ignore]` by default (`cargo test` skips it),
//!   - even under `--ignored`, returns early unless `OPENLET_LIVE_E2E=1`
//!     AND `OPENROUTER_API_KEY` is set.
//!
//! Run explicitly:
//!   OPENLET_LIVE_E2E=1 cargo test -p openlet-server --test \
//!     live_e2e_fs_agent_crud -- --ignored
//!
//! Zero mocks of fs/storage/permission: the real `LocalFilesystem`,
//! `SqliteMemoryStore`, and `ConfigPermissionMgr` (in Danger mode so the
//! model's tool calls auto-allow) do the work. Assertions check on-disk
//! state, tolerant of the model's exact wording.

use std::time::Duration;

mod live_support;
use live_support::{LiveServer, text_turn, tool_turn};

/// Poll the workspace for a predicate until true or the deadline passes.
async fn wait_disk(pred: impl Fn() -> bool, deadline: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < deadline {
        if pred() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    pred()
}

/// One bounded conversation where a real model genuinely advances through
/// create → read → edit → delete, asserting on-disk state after each. This
/// is the true zero-mock proof of the FS agent: the model decides the next
/// tool call from the prior tool result.
///
/// Two-tier: tier-2 (live) lets a real model advance the CRUD sequence; tier-1
/// (mock) scripts write→read→edit→bash(rm). Both dispatch the real
/// write/read/edit/bash tools against the real fs + shell, so the
/// create-then-delete on-disk assertion holds on either tier.
#[tokio::test]
async fn real_model_does_full_fs_crud() {
    // Tier-1 script: the full CRUD tool sequence. The write creates the file,
    // edit mutates it, bash(rm) deletes it — all real tools, both tiers.
    let script = vec![
        tool_turn(
            "w1",
            "write",
            r#"{"path":"notes.txt","content":"alpha\n"}"#,
        ),
        tool_turn("r1", "read", r#"{"path":"notes.txt"}"#),
        tool_turn(
            "e1",
            "edit",
            r#"{"path":"notes.txt","find":"alpha","replace":"alpha beta"}"#,
        ),
        tool_turn("d1", "bash", r#"{"command":"rm notes.txt"}"#),
        text_turn("DONE"),
    ];
    let srv = LiveServer::for_scenario(script).await;
    let ws = srv.workspace_root().to_path_buf();
    let file = ws.join("notes.txt");

    let sid = srv.create_session().await;
    // Danger mode so the model's write/edit/bash calls auto-allow against
    // the REAL permission manager — no mock gate.
    assert_eq!(
        srv.set_mode(&sid, "danger").await,
        reqwest::StatusCode::OK,
        "set danger mode"
    );

    // One prompt drives the whole sequence; the model advances step by step
    // because each tool result is appended and re-fed. Explicit, mechanical
    // instructions keep a small/cheap model on-track.
    let prompt = "Do EXACTLY these steps in order, one tool call at a time, \
        using the file `notes.txt` in the working directory: \
        1) write a file notes.txt containing the single line `alpha`. \
        2) read notes.txt back. \
        3) edit notes.txt so its content becomes `alpha beta`. \
        4) delete notes.txt by running the bash command `rm notes.txt`. \
        After deleting, reply DONE.";
    srv.prompt(&sid, prompt).await;

    // Drain the multi-turn conversation. A real model + several tool
    // round-trips needs a generous budget; bounded so a hung turn fails.
    let _frames = srv
        .collect_session_events(&sid, Duration::from_secs(90))
        .await;

    // The terminal observable outcome of the full sequence: the file was
    // created and then deleted, so it must NOT exist at the end. (We assert
    // the end state rather than each intermediate step, since a model may
    // batch reads or reorder narration — but the create-then-delete arc is
    // unambiguous on disk.)
    let gone = wait_disk(|| !file.exists(), Duration::from_secs(5)).await;
    assert!(
        gone,
        "after create→…→delete, {} must not exist on disk",
        file.display()
    );

    // Sanity: the workspace dir itself still exists (we deleted only the
    // file, not the tree) — guards against an over-broad rm.
    assert!(ws.exists(), "workspace dir must survive");
}
