//! Gated real-OpenRouter debug→fix→verify E2E — proves a real model can run
//! BROKEN code, diagnose it from its own runtime traceback, repair it, and
//! re-run to confirm the fix.
//!
//! This exercises the loop a human engineer actually runs: run the code →
//! observe the failure → fix the bug → run again to verify green. The model is
//! NOT told what the bug is; it must infer it from the traceback. The
//! deterministic mock cannot do this — only a real model reacts to a tool
//! result it couldn't predict.
//!
//! Phase 7 cutover rewrite. WAS: `python3` invoked through the subprocess
//! `bash` tool. The emulated shell has no `python3` binary by construction
//! (any external command → `command not found`), so the debug loop now runs on
//! the in-process `python` tool (Monty). Per the plan, the loop is INLINE:
//! "LLM writes buggy code → python runs → reads the traceback → fixes → runs
//! again". Monty is computation-only and cannot `exec()` a file (a nested
//! `open()` OsCall inside `exec` isn't resumable), so the program IS the tool
//! call's code — the natural Monty workflow. Each run still persists its answer
//! to disk through `ctx.fs`, so the on-disk assertion stays meaningful.
//!
//! Gated identically to the other live tiers: the runtime env gate
//! (`LETI_LIVE_E2E=1` + `OPENAI_API_KEY`) selects the real provider; unset,
//! the harness falls back to the scripted mock so `cargo test` makes no network
//! calls.
//!
//! Run against real OpenRouter:
//!   LETI_LIVE_E2E=1 OPENAI_API_KEY=... \
//!     cargo test -p leti-server --test live_e2e_debug_fix_verify
//!
//! Zero mocks: real `MontyExecutor` runs the code in-process, real
//! `LocalFilesystem` holds the artifact, real permission mgr in Danger mode.
//! Assertion checks the on-disk result is correct AND that the repaired code
//! actually executes clean through the SAME executor the model used.

use std::sync::Arc;
use std::time::Duration;

mod live_support;
use live_support::{LiveServer, text_turn, tool_turn};

use leti_adapters::localfs::LocalFilesystem;
use leti_adapters::pyexec::MontyExecutor;
use leti_core::tools::builtins::python::PythonExecutor;

/// The corrected program the loop must converge on: sum [1..5] and persist the
/// answer to `answer.txt` through the fs seam. Used as the tier-1 "fixed" turn
/// AND re-run independently below to prove it executes clean (not circular: the
/// re-run uses a FRESH executor + a fresh read of on-disk state).
const FIXED_CODE: &str = "def add_all(nums):\n    \
    result = 0\n    \
    for n in nums:\n        \
    result += n\n    \
    return result\n\
    open('answer.txt', 'w').write(str(add_all([1, 2, 3, 4, 5])))\n\
    print(add_all([1, 2, 3, 4, 5]))\n";

/// Same program with the runtime-bug typo (`result` computed, `reslt` returned)
/// → NameError traceback the model must read to locate the fix.
const BUGGY_CODE: &str = "def add_all(nums):\n    \
    result = 0\n    \
    for n in nums:\n        \
    result += n\n    \
    return reslt\n\
    open('answer.txt', 'w').write(str(add_all([1, 2, 3, 4, 5])))\n\
    print(add_all([1, 2, 3, 4, 5]))\n";

async fn wait_disk(pred: impl Fn() -> bool, deadline: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < deadline {
        if pred() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    pred()
}

/// Run `code` through a FRESH Monty executor rooted at `workspace`. Used for
/// the final independent-verification step so the proof isn't circular with the
/// model's own run.
async fn run_via_monty(workspace: &std::path::Path, code: &str) -> (i32, String, String) {
    let exec = MontyExecutor::new();
    let ctx =
        live_support::minimal_tool_ctx(Arc::new(LocalFilesystem::new(workspace.to_path_buf())));
    let out = exec
        .run(&ctx, code, 5_000)
        .await
        .expect("monty run returns Ok(PythonOutput)");
    (out.exit_code, out.stdout, out.stderr)
}

/// The model runs buggy code via the `python` tool, reads the NameError
/// traceback, and re-runs corrected code that both prints 15 and persists it to
/// `answer.txt`. Final assertions: the on-disk answer is 15 AND the repaired
/// code executes clean through a fresh executor.
///
/// Two-tier: tier-2 (live) lets a real model decide the run→diagnose→fix→run
/// sequence; tier-1 (mock) scripts that exact sequence. BOTH dispatch the real
/// `python` tool against the real executor + fs, so the on-disk + re-run
/// assertions are meaningful on either tier.
#[tokio::test]
async fn real_model_debugs_and_fixes_runtime_error() {
    // Tier-1 script: buggy run → fixed run → DONE. The fixed turn carries the
    // REAL correction; the python tool executes it on both tiers, so this drives
    // the same wiring rather than being a tautology.
    let script = vec![
        tool_turn(
            "c1",
            "python",
            &serde_json::json!({ "code": BUGGY_CODE }).to_string(),
        ),
        tool_turn(
            "c2",
            "python",
            &serde_json::json!({ "code": FIXED_CODE }).to_string(),
        ),
        text_turn("DONE"),
    ];
    let srv = LiveServer::for_scenario(script).await;
    let ws = srv.workspace_root().to_path_buf();
    let answer_path = ws.join("answer.txt");

    let sid = srv.create_session().await;
    assert_eq!(
        srv.set_mode(&sid, "danger").await,
        reqwest::StatusCode::OK,
        "set danger mode"
    );

    let prompt = "Compute the sum of [1, 2, 3, 4, 5] (which is 15) and write it to \
        `answer.txt`, then print it. Use the python tool. It is currently \
        buggy and raises an error — run it, read the traceback, fix the bug, \
        and run it again until it writes 15 to answer.txt and prints 15 with \
        no error. Do NOT shell out; use the python tool only. When it succeeds, \
        reply DONE.";
    srv.prompt(&sid, prompt).await;

    let _frames = srv
        .collect_session_events(&sid, Duration::from_secs(120))
        .await;

    let read_answer = || std::fs::read_to_string(&answer_path).unwrap_or_default();

    // Invariant 1: the corrected run persisted 15 to disk through the fs seam.
    let wrote_15 = wait_disk(|| read_answer().trim() == "15", Duration::from_secs(8)).await;
    assert!(
        wrote_15,
        "the python debug loop must persist 15 to answer.txt. Content now: {:?}",
        read_answer()
    );

    // Invariant 2 — the real proof: run the repaired program through a FRESH
    // Monty executor. It must exit 0, print 15, and (re)write answer.txt = 15.
    // This catches a "fix" that happened to leave a stale correct file but no
    // longer runs, AND proves it on the same in-process executor the production
    // agent ships (no host `python3` dependency).
    let (exit_code, stdout, stderr) = run_via_monty(&ws, FIXED_CODE).await;
    assert_eq!(
        exit_code, 0,
        "repaired program must run clean through Monty. stderr:\n{stderr}"
    );
    assert_eq!(stdout.trim(), "15", "repaired program must print 15");
    assert_eq!(
        read_answer().trim(),
        "15",
        "repaired program must persist 15 to answer.txt via ctx.fs"
    );
}
