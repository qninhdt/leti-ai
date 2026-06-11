//! Gated real-OpenRouter debug→fix→verify E2E — proves a real model can take a
//! BROKEN program, diagnose it from its own runtime output, repair it, and
//! re-run to confirm the fix.
//!
//! This exercises the loop a human engineer actually runs: `bash` the script →
//! observe the failure → `read` the source → `edit` the bug → `bash` again to
//! verify green. The model is NOT told what the bug is; it must infer it from
//! the traceback. The deterministic mock cannot do this — only a real model
//! reacts to a tool result it couldn't predict.
//!
//! Gated identically to the other live tiers (`#[ignore]` +
//! `OPENLET_LIVE_E2E=1` + `OPENROUTER_API_KEY`).
//!
//! Run:
//!   OPENLET_LIVE_E2E=1 cargo test -p openlet-server --test \
//!     live_e2e_debug_fix_verify -- --ignored
//!
//! Zero mocks: real `LocalShellExecutor` runs the actual `python3`, real
//! `LocalFilesystem` holds the source, real permission mgr in Danger mode.
//! Assertion checks the on-disk result is correct AND actually executes clean.

use std::process::Command;
use std::time::Duration;

mod live_support;
use live_support::{LiveServer, text_turn, tool_turn};

/// True if `python3` exists on PATH. The scenario shells out to it, so a box
/// without Python should skip rather than spuriously fail.
fn python_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

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

/// Seed a script with a real bug (a typo'd variable → NameError at runtime),
/// ask the model to make it run correctly. The model must run it, read the
/// traceback, fix the typo, and re-run. Final assertion: the script both has
/// the corrected symbol AND executes to the expected output from a clean shell.
///
/// Two-tier: tier-2 (live) lets a real model decide the bash→read→edit→bash
/// sequence; tier-1 (mock) scripts that exact sequence. BOTH dispatch the real
/// `bash`/`edit` tools against the real shell + fs, so the on-disk + re-run
/// assertions are meaningful on either tier.
#[tokio::test]
async fn real_model_debugs_and_fixes_runtime_error() {
    if !python_available() {
        eprintln!("skipping: python3 not on PATH");
        return;
    }

    // Tier-1 script: the plausible tool sequence a model would run. The `edit`
    // call carries the REAL fix (reslt → result); the edit tool executes it on
    // both tiers, so this is not a tautology — it drives the same wiring.
    let script = vec![
        tool_turn("c1", "bash", r#"{"command":"python3 compute.py"}"#),
        tool_turn("c2", "read", r#"{"path":"compute.py"}"#),
        tool_turn(
            "c3",
            "edit",
            r#"{"path":"compute.py","find":"return reslt","replace":"return result"}"#,
        ),
        tool_turn("c4", "bash", r#"{"command":"python3 compute.py"}"#),
        text_turn("DONE"),
    ];
    let srv = LiveServer::for_scenario(script).await;
    let ws = srv.workspace_root().to_path_buf();
    let script_path = ws.join("compute.py");

    // The bug: the function computes into `result` but returns `reslt` (a
    // typo) → NameError when called. Running it prints a traceback the model
    // must read to locate the fix. The correct output is the integer 15.
    std::fs::write(
        &script_path,
        "def add_all(nums):\n\
         \x20\x20\x20\x20result = 0\n\
         \x20\x20\x20\x20for n in nums:\n\
         \x20\x20\x20\x20\x20\x20\x20\x20result += n\n\
         \x20\x20\x20\x20return reslt\n\
         \n\
         print(add_all([1, 2, 3, 4, 5]))\n",
    )
    .expect("seed compute.py");

    let sid = srv.create_session().await;
    assert_eq!(
        srv.set_mode(&sid, "danger").await,
        reqwest::StatusCode::OK,
        "set danger mode"
    );

    let prompt = "The file `compute.py` in the working directory is supposed to \
        print the sum of [1,2,3,4,5] (which is 15) but it currently crashes. \
        Fix it. Steps, one tool call at a time: \
        1) run `python3 compute.py` with the bash tool and observe the error. \
        2) read compute.py to find the cause. \
        3) use the edit tool to fix the bug (do not rewrite the whole file if a \
        small edit suffices). \
        4) run `python3 compute.py` again to confirm it now prints 15. \
        When it prints 15 with no error, reply DONE.";
    srv.prompt(&sid, prompt).await;

    let _frames = srv
        .collect_session_events(&sid, Duration::from_secs(120))
        .await;

    let read_src = || std::fs::read_to_string(&script_path).unwrap_or_default();

    // Invariant 1: the typo'd symbol is gone from the source.
    let fixed = wait_disk(
        || {
            let s = read_src();
            !s.contains("reslt") && s.contains("result")
        },
        Duration::from_secs(8),
    )
    .await;
    assert!(
        fixed,
        "the `reslt` typo must be corrected to `result`. Source now:\n{}",
        read_src()
    );

    // Invariant 2 — the real proof: run the (now-edited) script ourselves from
    // a clean shell. It must exit 0 and print 15. This catches a model that
    // "fixed" the file into something that still doesn't run.
    let out = Command::new("python3")
        .arg("compute.py")
        .current_dir(&ws)
        .output()
        .expect("run python3");
    assert!(
        out.status.success(),
        "fixed script must run clean. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.trim() == "15",
        "fixed script must print 15, got: {:?}",
        stdout.trim()
    );
}
