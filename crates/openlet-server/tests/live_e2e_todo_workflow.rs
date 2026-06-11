//! Gated real-OpenRouter capstone E2E — a long, realistic agentic workflow
//! that uses the `todo` tool to track multi-step work WHILE doing it.
//!
//! This is the closest thing to a real engineering session: the model is asked
//! to (1) maintain a todo list across the task, (2) write a Python module,
//! (3) write a test for it, (4) run the test with bash, and (5) mark todos
//! complete as it goes. It exercises `todo` + `write` + `bash` + multi-turn
//! reasoning together — not one tool in isolation. The `todo` tool persists a
//! durable `todos.json` artifact, which we read back as the proof that the
//! checklist was actually maintained (not just narrated).
//!
//! Gated identically to the other live tiers (`#[ignore]` +
//! `OPENLET_LIVE_E2E=1` + `OPENROUTER_API_KEY`).
//!
//! Run:
//!   OPENLET_LIVE_E2E=1 cargo test -p openlet-server --test \
//!     live_e2e_todo_workflow -- --ignored

use std::process::Command;
use std::time::Duration;

mod live_support;
use live_support::LiveServer;

fn live_enabled() -> bool {
    std::env::var("OPENLET_LIVE_E2E").as_deref() == Ok("1")
        && std::env::var("OPENROUTER_API_KEY").is_ok()
}

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
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    pred()
}

/// A real model runs a small end-to-end build task while maintaining a todo
/// list. Assertions: the module + test landed on disk, the test actually
/// passes when WE run it independently, and a durable todos.json was persisted
/// recording the checklist the model maintained.
#[tokio::test]
#[ignore = "live OpenRouter; run with OPENLET_LIVE_E2E=1 -- --ignored"]
async fn real_model_runs_todo_tracked_build_workflow() {
    if !live_enabled() {
        eprintln!("skipping: set OPENLET_LIVE_E2E=1 + OPENROUTER_API_KEY to run");
        return;
    }
    if !python_available() {
        eprintln!("skipping: python3 not on PATH");
        return;
    }

    let srv = LiveServer::with_openrouter().await;
    let ws = srv.workspace_root().to_path_buf();
    let module = ws.join("mathutils.py");
    let test_file = ws.join("test_mathutils.py");

    let sid = srv.create_session().await;
    assert_eq!(
        srv.set_mode(&sid, "danger").await,
        reqwest::StatusCode::OK,
        "set danger mode"
    );

    let prompt = "You are doing a small build task. Maintain a todo list with \
        the `todo` tool throughout (create it up front with the steps, and \
        update item statuses to completed as you finish each). The task: \
        1) write `mathutils.py` defining `def factorial(n):` that returns n! \
        (factorial; factorial(5) must be 120, factorial(0) must be 1). \
        2) write `test_mathutils.py` that imports factorial and checks \
        factorial(5)==120 and factorial(0)==1, printing `ALL_TESTS_PASSED` if \
        both hold. \
        3) run `python3 test_mathutils.py` with the bash tool and confirm it \
        prints ALL_TESTS_PASSED. \
        Update your todo list to mark items completed as you go. When the test \
        passes, reply DONE.";
    let ack = srv.prompt(&sid, prompt).await;
    assert_eq!(ack, reqwest::StatusCode::ACCEPTED, "prompt ack");

    // A multi-tool build+test+todo workflow is a long multi-turn run.
    let _frames = srv
        .collect_session_events(&sid, Duration::from_secs(180))
        .await;

    // Invariant 1: both files were created.
    let built = wait_disk(
        || module.exists() && test_file.exists(),
        Duration::from_secs(8),
    )
    .await;
    assert!(
        built,
        "expected mathutils.py + test_mathutils.py on disk"
    );

    // Invariant 2 — the real proof: run the model's test ourselves from a
    // clean shell. It must pass against the module the model wrote.
    let out = Command::new("python3")
        .arg("test_mathutils.py")
        .current_dir(&ws)
        .output()
        .expect("run python3 test");
    assert!(
        out.status.success(),
        "the model's own test must pass when run independently. stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("ALL_TESTS_PASSED"),
        "test must print the success sentinel, got: {stdout:?}"
    );

    // Sanity: factorial is actually correct, run directly (guards a test that
    // passes vacuously). 5! = 120.
    let direct = Command::new("python3")
        .arg("-c")
        .arg("import mathutils; print(mathutils.factorial(5))")
        .current_dir(&ws)
        .output()
        .expect("run factorial directly");
    assert_eq!(
        String::from_utf8_lossy(&direct.stdout).trim(),
        "120",
        "factorial(5) must be 120"
    );

    // Invariant 3 — the todo proof: the `todo` tool persisted a durable
    // todos.json artifact. Read it back from the same store the server uses
    // and assert it is a non-empty list whose items carry the expected shape
    // (content + status), proving the checklist was genuinely maintained.
    let raw = srv.read_artifact(&sid, "todos.json").await;
    let raw = raw.expect("todo tool must have persisted todos.json");
    let parsed: serde_json::Value =
        serde_json::from_slice(&raw).expect("todos.json must be valid JSON");
    let items = parsed.as_array().expect("todos.json is a JSON array");
    assert!(
        !items.is_empty(),
        "the maintained todo list must be non-empty"
    );
    // Every item carries content + a known status — the tool's schema.
    for item in items {
        assert!(
            item.get("content").and_then(serde_json::Value::as_str).is_some(),
            "each todo needs content: {item:?}"
        );
        let status = item
            .get("status")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        assert!(
            matches!(
                status,
                "pending" | "in_progress" | "completed" | "cancelled"
            ),
            "each todo needs a valid status, got {status:?}"
        );
    }
    // At least one item should be marked completed — the model was told to
    // update statuses as it finished steps, and the task DID complete.
    let any_completed = items.iter().any(|i| {
        i.get("status").and_then(serde_json::Value::as_str) == Some("completed")
    });
    assert!(
        any_completed,
        "at least one todo should be marked completed after the task finished: {items:?}"
    );
}
