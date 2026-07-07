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
//! Gated identically to the other live tiers: the runtime env gate
//! (`OPENLET_LIVE_E2E=1` + `OPENAI_API_KEY`) selects the real provider;
//! unset, the harness falls back to the scripted mock so `cargo test` makes no
//! network calls.
//!
//! Run against real OpenRouter:
//!   OPENLET_LIVE_E2E=1 OPENAI_API_KEY=... \
//!     cargo test -p openlet-server --test live_e2e_todo_workflow

use std::process::Command;
use std::time::Duration;

mod live_support;
use live_support::{LiveServer, text_turn, tool_turn};

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
///
/// Two-tier: tier-2 (live) lets a real model build+test while tracking todos;
/// tier-1 (mock) scripts todo→write→write→bash→todo. Both dispatch the real
/// todo/write/bash tools, so the on-disk module/test + persisted todos.json
/// assertions are meaningful on either tier.
#[tokio::test]
async fn real_model_runs_todo_tracked_build_workflow() {
    if !python_available() {
        eprintln!("skipping: python3 not on PATH");
        return;
    }

    // Tier-1 script: create a todo list, write the module + test, run the
    // test, then mark todos completed. The writes carry real content the
    // assertions check; the todo calls persist the real todos.json. All tools
    // execute on both tiers.
    let todos_initial = r#"{"todos":[{"content":"write mathutils.py","status":"in_progress","priority":"high"},{"content":"write test","status":"pending","priority":"high"},{"content":"run test","status":"pending","priority":"medium"}]}"#;
    let todos_done = r#"{"todos":[{"content":"write mathutils.py","status":"completed","priority":"high"},{"content":"write test","status":"completed","priority":"high"},{"content":"run test","status":"completed","priority":"medium"}]}"#;
    let module_src = r#"{"path":"mathutils.py","content":"def factorial(n):\n    return 1 if n <= 1 else n * factorial(n - 1)\n"}"#;
    let test_src = r#"{"path":"test_mathutils.py","content":"from mathutils import factorial\nassert factorial(5) == 120\nassert factorial(0) == 1\nprint('ALL_TESTS_PASSED')\n"}"#;
    let script = vec![
        tool_turn("t1", "todo", todos_initial),
        tool_turn("w1", "write", module_src),
        tool_turn("w2", "write", test_src),
        tool_turn("b1", "bash", r#"{"command":"python3 test_mathutils.py"}"#),
        tool_turn("t2", "todo", todos_done),
        text_turn("DONE"),
    ];
    let srv = LiveServer::for_scenario(script).await;
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
    assert!(built, "expected mathutils.py + test_mathutils.py on disk");

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
            item.get("content")
                .and_then(serde_json::Value::as_str)
                .is_some(),
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
    let any_completed = items
        .iter()
        .any(|i| i.get("status").and_then(serde_json::Value::as_str) == Some("completed"));
    assert!(
        any_completed,
        "at least one todo should be marked completed after the task finished: {items:?}"
    );
}
