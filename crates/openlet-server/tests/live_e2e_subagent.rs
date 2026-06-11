//! Gated real-OpenRouter subagent E2E — the multi-agent orchestration proof.
//!
//! `subagent_task` is the headline orchestration feature: a parent model
//! delegates a sub-objective to a child agent, the server's REAL
//! `RuntimeSubagentSpawner` admits it (depth + per-root quota), persists a
//! child session, seeds the objective, and drives a NESTED `run_loop`; the
//! child's output + cost roll back up to the parent. Every other live test
//! stubs the spawner (`StubSubagentSpawner` returns `Err`); this one boots via
//! `with_openrouter_subagents`, which wires the real spawner late-bound to
//! AppState exactly as `main.rs` does — so a real model on BOTH the parent and
//! the child turn drives the whole arc.
//!
//! Gated identically to the other live tiers (`#[ignore]` +
//! `OPENLET_LIVE_E2E=1` + `OPENROUTER_API_KEY`).
//!
//! Run:
//!   OPENLET_LIVE_E2E=1 cargo test -p openlet-server --test \
//!     live_e2e_subagent -- --ignored

use std::time::Duration;

mod live_support;
use live_support::{LiveServer, text_turn, tool_turn};

async fn wait_disk(pred: impl Fn() -> bool, deadline: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < deadline {
        if pred() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    pred()
}

/// The parent model is told to DELEGATE a concrete file-writing objective to a
/// `general` subagent and wait for it. The child shares the parent's workspace
/// (the spawner clones the parent's AgentResources), so the file the child
/// writes lands on the same disk we inspect. The assertion is the delegated
/// work's on-disk result — only a real spawn→child-run→return arc produces it.
///
/// Two-tier: tier-2 (live) lets a real model on BOTH parent and child decide;
/// tier-1 (mock) scripts the interleaved turns. The scripted provider serves
/// parent + child from one queue; because the spawn is synchronous (the parent
/// awaits the child before its tool result returns), the pop order is
/// deterministic: parent-spawn → child-write → child-done → parent-done. The
/// REAL spawner drives a real nested child run_loop on both tiers.
#[tokio::test]
async fn real_model_delegates_to_subagent_that_does_the_work() {
    // Tier-1 script in synchronous-spawn pop order:
    //   1. parent: subagent_task(general, "...write subagent_proof.txt...")
    //   2. child:  write subagent_proof.txt
    //   3. child:  text DONE (ends the child run_loop)
    //   4. parent: text DONE (after the child result rolls up)
    let script = vec![
        tool_turn(
            "s1",
            "subagent_task",
            r#"{"subagent_type":"general","objective":"write subagent_proof.txt containing DELEGATED_WORK_DONE"}"#,
        ),
        tool_turn(
            "cw",
            "write",
            r#"{"path":"subagent_proof.txt","content":"DELEGATED_WORK_DONE\n"}"#,
        ),
        text_turn("child done"),
        text_turn("DONE"),
    ];
    // Boots with the REAL subagent spawner wired in (both tiers).
    let srv = LiveServer::for_scenario_with_subagents(script).await;
    let ws = srv.workspace_root().to_path_buf();
    let proof = ws.join("subagent_proof.txt");

    let sid = srv.create_session().await;
    // Danger mode cascades to the child session (child inherits the parent's
    // permission_mode at spawn), so the child's write auto-allows.
    assert_eq!(
        srv.set_mode(&sid, "danger").await,
        reqwest::StatusCode::OK,
        "set danger mode"
    );

    let prompt = "Delegate this task to a subagent instead of doing it \
        yourself. Use the subagent_task tool with subagent_type set to \
        `general` and an objective instructing the subagent to: write a file \
        named `subagent_proof.txt` in the working directory whose entire \
        contents are exactly the line `DELEGATED_WORK_DONE`. Wait for the \
        subagent to finish, then reply DONE. Do not write the file yourself — \
        the subagent must do it.";
    let ack = srv.prompt(&sid, prompt).await;
    assert_eq!(ack, reqwest::StatusCode::ACCEPTED, "prompt ack");

    // A nested child run_loop on top of the parent turn is slow; give it a
    // generous bounded budget so a genuine hang still fails the test.
    let _frames = srv
        .collect_session_events(&sid, Duration::from_secs(180))
        .await;

    // The proof: the delegated file exists with the exact sentinel content.
    // The parent was told NOT to write it, so its presence means the child
    // session actually ran and did the work.
    let landed = wait_disk(
        || {
            std::fs::read_to_string(&proof)
                .map(|s| s.contains("DELEGATED_WORK_DONE"))
                .unwrap_or(false)
        },
        Duration::from_secs(10),
    )
    .await;
    assert!(
        landed,
        "subagent must have written {} with the sentinel; contents: {:?}",
        proof.display(),
        std::fs::read_to_string(&proof).ok()
    );

    // A child session was persisted (depth 1) — corroborates that the spawn
    // actually created a nested session rather than the parent doing the work
    // inline. We assert at least 2 sessions exist (parent + >=1 child).
    let sessions = srv.get_json("/v1/session").await;
    let count = sessions.as_array().map(Vec::len).unwrap_or(0);
    assert!(
        count >= 2,
        "expected a parent + at least one spawned child session, saw {count}"
    );
}
