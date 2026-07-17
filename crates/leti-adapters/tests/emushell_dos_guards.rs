//! Phase 5 DoS guards for the emulated shell.
//!
//! The interpreter runs in-process, so — unlike the old subprocess executor —
//! a wall-clock `tokio::time::timeout` cannot by itself pre-empt a tight
//! `while true` loop that never `.await`s the filesystem. These tests prove
//! the three in-band guards close that gap:
//!   - a pure-CPU infinite loop is bounded by the wall-clock deadline (never
//!     hangs the runtime), and reports `timed_out`;
//!   - a cancel token fired mid-run stops the loop cooperatively and surfaces
//!     as `Err(ToolError::Timeout)`;
//!   - a normal, finite workload is NOT cut and completes with exit 0.
//!
//! All run under the multi-thread scheduler so a monopolized worker cannot mask
//! a missing yield point.

mod common;

use std::time::{Duration, Instant};

use common::tempdir_workspace::WorkspaceFixture;
use common::tool_ctx_harness::tool_ctx;
use leti_adapters::emushell::EmulatedShellExecutor;
use leti_core::error::ToolError;
use leti_core::tools::builtins::bash::ShellExecutor;
use tokio_util::sync::CancellationToken;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn infinite_loop_is_bounded_by_wall_clock() {
    let fx = WorkspaceFixture::empty();
    let exec = EmulatedShellExecutor::new();
    let ctx = tool_ctx(fx.root(), CancellationToken::new());

    let start = Instant::now();
    // A tight loop whose body (`:` / true) never touches the filesystem, so
    // nothing in it `.await`s — only the in-band deadline can stop it.
    let out = exec
        .run(&ctx, "while true; do :; done", 500)
        .await
        .expect("timeout must return Ok(BashOutput), not Err");
    let elapsed = start.elapsed();

    assert!(out.timed_out, "expected timed_out flag set");
    assert_ne!(out.exit_code, 0, "timed-out run must be non-zero exit");
    // Either DoS guard is an acceptable stop for an infinite loop: the
    // wall-clock deadline normally fires first, but a fast (release) build
    // could burn the 5M step budget before 500ms elapses. Both set
    // `timed_out` and emit a naming stderr line — assert one of them fired
    // rather than coupling the test to which guard won the race.
    assert!(
        out.stderr.contains("wall-clock timeout") || out.stderr.contains("step budget"),
        "stderr should name the guard that fired: {}",
        out.stderr
    );
    // Generous ceiling: proves it actually stopped (near the 500ms deadline or
    // at the step budget) rather than hanging the runtime indefinitely.
    assert!(
        elapsed < Duration::from_secs(5),
        "loop ran {elapsed:?}, expected it bounded by a DoS guard"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_stops_loop_mid_run() {
    let fx = WorkspaceFixture::empty();
    let exec = EmulatedShellExecutor::new();
    let cancel = CancellationToken::new();
    let ctx = tool_ctx(fx.root(), cancel.clone());

    // Fire cancellation shortly after the run starts; the loop must observe it
    // cooperatively (via the periodic yield + tick check) and stop well before
    // the 60s timeout would.
    let canceller = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        canceller.cancel();
    });

    let start = Instant::now();
    let result = exec.run(&ctx, "while true; do :; done", 60_000).await;
    let elapsed = start.elapsed();

    // Cancellation (not a resource guard) surfaces as Err(Timeout), matching
    // the subprocess executor's cancel contract.
    assert!(
        matches!(result, Err(ToolError::Timeout)),
        "cancel should surface as Err(ToolError::Timeout), got {result:?}"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "cancel took {elapsed:?}, expected it to stop promptly, not wait out the 60s timeout"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn finite_workload_is_not_cut() {
    let fx = WorkspaceFixture::empty();
    let exec = EmulatedShellExecutor::new();
    let ctx = tool_ctx(fx.root(), CancellationToken::new());

    // A reasonable finite loop must complete untouched by either guard.
    let out = exec
        .run(&ctx, "for i in 1 2 3 4 5; do echo $i; done", 5_000)
        .await
        .expect("finite loop must return Ok(BashOutput)");

    assert!(
        !out.timed_out,
        "finite workload must NOT be flagged timed_out"
    );
    assert_eq!(out.exit_code, 0, "finite workload should exit 0");
    assert_eq!(out.stdout, "1\n2\n3\n4\n5\n");
    assert!(
        !out.stderr.contains("timeout") && !out.stderr.contains("budget"),
        "finite workload must not trip a guard: {}",
        out.stderr
    );
}
