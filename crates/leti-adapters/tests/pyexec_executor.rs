//! `MontyExecutor` integration — proves the Phase 4 success criteria: the
//! in-process Python interpreter runs computation, routes every file op
//! through `ctx.fs`, denies process/network/host-env access by construction,
//! cannot escape the workspace, and stays alive under resource-bomb code.
//!
//! Mirrors `emushell_interpreter.rs` in shape: build a `ToolCtx` rooted at a
//! tempdir workspace, run a snippet, assert on the `PythonOutput`.

mod common;

use std::path::Path;

use common::tempdir_workspace::WorkspaceFixture;
use common::tool_ctx_harness::tool_ctx;
use leti_adapters::pyexec::MontyExecutor;
use leti_core::tools::builtins::python::{PythonExecutor, PythonOutput};
use tokio_util::sync::CancellationToken;

/// Run `code` against a workspace seeded with `files`.
async fn run_in(files: &[(&str, &str)], code: &str) -> PythonOutput {
    let owned: Vec<(String, String)> = files
        .iter()
        .map(|(p, c)| ((*p).to_string(), (*c).to_string()))
        .collect();
    let fx = WorkspaceFixture::with_files(owned.iter().map(|(p, c)| (p.as_str(), c.as_str())));
    run_at(fx.root(), code).await
}

async fn run_at(root: &Path, code: &str) -> PythonOutput {
    let exec = MontyExecutor::new();
    let ctx = tool_ctx(root, CancellationToken::new());
    exec.run(&ctx, code, 5_000)
        .await
        .expect("executor run should return Ok(PythonOutput), never Err")
}

// ---------------------------------------------------------------------------
// (1) Computation fidelity — arithmetic, functions, comprehensions, json, re.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn arithmetic_last_expression_echoes() {
    let out = run_in(&[], "2 ** 10").await;
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert_eq!(out.stdout.trim(), "1024");
}

#[tokio::test]
async fn recursive_function_and_comprehension() {
    let out = run_in(
        &[],
        "def fib(n):\n    return n if n < 2 else fib(n-1)+fib(n-2)\nsum([fib(i) for i in range(10)])",
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    // fib(0..9) = 0,1,1,2,3,5,8,13,21,34 -> sum 88
    assert_eq!(out.stdout.trim(), "88");
}

#[tokio::test]
async fn json_and_regex_modules() {
    let json = run_in(&[], "import json\njson.loads('{\"n\": 3}')['n']").await;
    assert_eq!(json.exit_code, 0, "stderr: {}", json.stderr);
    assert_eq!(json.stdout.trim(), "3");

    let re = run_in(&[], "import re\nre.findall(r'\\d+', 'a1b22c333')").await;
    assert_eq!(re.exit_code, 0, "stderr: {}", re.stderr);
    assert_eq!(re.stdout.trim(), "['1', '22', '333']");
}

#[tokio::test]
async fn print_output_is_captured() {
    let out = run_in(&[], "print('hello')\nprint('world')").await;
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert_eq!(out.stdout, "hello\nworld\n");
}

// ---------------------------------------------------------------------------
// (2) IO seam — open()/pathlib route through ctx.fs, never host disk.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn open_read_routes_through_fs() {
    let out = run_in(&[("in.txt", "hello\nworld\n")], "open('in.txt').read()").await;
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert_eq!(out.stdout.trim_end(), "hello\nworld");
}

#[tokio::test]
async fn pathlib_read_text_routes_through_fs() {
    let out = run_in(
        &[("data.txt", "payload")],
        "from pathlib import Path\nPath('data.txt').read_text()",
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert_eq!(out.stdout.trim_end(), "payload");
}

#[tokio::test]
async fn open_write_persists_through_fs() {
    let fx = WorkspaceFixture::empty();
    let out = run_at(
        fx.root(),
        "with open('out.txt', 'w') as f:\n    f.write('written-by-monty')",
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    // Readback through the real host FS confirms the write hit ctx.fs.
    let disk = std::fs::read_to_string(fx.root().join("out.txt")).expect("file written");
    assert_eq!(disk, "written-by-monty");
}

#[tokio::test]
async fn pathlib_write_text_persists_through_fs() {
    let fx = WorkspaceFixture::empty();
    let out = run_at(
        fx.root(),
        "from pathlib import Path\nPath('note.txt').write_text('via-pathlib')",
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    let disk = std::fs::read_to_string(fx.root().join("note.txt")).expect("file written");
    assert_eq!(disk, "via-pathlib");
}

#[tokio::test]
async fn multi_write_on_truncating_handle_appends() {
    // Monty flips `first_write_done` after the first write on a `w` handle,
    // so the 2nd+ write surfaces as AppendText. The bridge must honor that,
    // otherwise the second write would truncate the first.
    let fx = WorkspaceFixture::empty();
    let out = run_at(
        fx.root(),
        "with open('log.txt', 'w') as f:\n    f.write('a')\n    f.write('b')\n    f.write('c')",
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    let disk = std::fs::read_to_string(fx.root().join("log.txt")).expect("file written");
    assert_eq!(disk, "abc");
}

#[tokio::test]
async fn exists_check_via_fs() {
    let out = run_in(
        &[("present.txt", "x")],
        "from pathlib import Path\n(Path('present.txt').exists(), Path('absent.txt').exists())",
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert_eq!(out.stdout.trim(), "(True, False)");
}

#[tokio::test]
async fn round_trip_read_transform_write() {
    let fx = WorkspaceFixture::with_files([("src.txt", "hello world")]);
    let out = run_at(
        fx.root(),
        "data = open('src.txt').read().upper()\nopen('dst.txt', 'w').write(data)",
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    let disk = std::fs::read_to_string(fx.root().join("dst.txt")).expect("file written");
    assert_eq!(disk, "HELLO WORLD");
}

// ---------------------------------------------------------------------------
// (3) Deny-by-default — no process, no network, no host env.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn os_system_is_denied() {
    let out = run_in(&[], "import os\nos.system('id')").await;
    assert_ne!(out.exit_code, 0, "os.system must not succeed");
    assert!(!out.stderr.is_empty(), "expected an error on stderr");
}

#[tokio::test]
async fn import_socket_is_denied() {
    let out = run_in(&[], "import socket\nsocket.socket()").await;
    assert_ne!(out.exit_code, 0, "socket must be unavailable");
}

#[tokio::test]
async fn import_subprocess_is_denied() {
    let out = run_in(&[], "import subprocess\nsubprocess.run(['id'])").await;
    assert_ne!(out.exit_code, 0, "subprocess must be unavailable");
}

// ---------------------------------------------------------------------------
// (4) No workspace escape.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn absolute_host_path_not_readable() {
    // /etc/hostname exists on the host but is outside the workspace root, so
    // the FS seam must refuse it rather than leaking host disk.
    let out = run_in(&[], "open('/etc/hostname').read()").await;
    assert_ne!(out.exit_code, 0, "host path must not be readable");
    assert!(
        out.stderr.contains("FileNotFoundError")
            || out.stderr.contains("PermissionError")
            || out.stderr.contains("No such file"),
        "expected a not-found / permission error, got: {}",
        out.stderr
    );
}

#[tokio::test]
async fn parent_traversal_is_denied() {
    let out = run_in(&[], "open('../escape.txt', 'w').write('x')").await;
    assert_ne!(out.exit_code, 0, "parent-dir escape must be denied");
}

// ---------------------------------------------------------------------------
// (5) Resource limits — process must survive the bomb.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn memory_bomb_trips_limit_process_survives() {
    let fx = WorkspaceFixture::empty();
    let exec = MontyExecutor::new().with_max_memory(4 * 1024 * 1024);
    let ctx = tool_ctx(fx.root(), CancellationToken::new());
    let out = exec
        .run(&ctx, "x = [0] * (10 ** 12)\nlen(x)", 5_000)
        .await
        .expect("run returns Ok even when the guest hits a limit");
    assert_ne!(out.exit_code, 0, "memory bomb must fail the run");
    // The real proof is that we reached this assertion at all — the host
    // process is still alive to run it.
}

#[tokio::test]
async fn infinite_loop_trips_timeout_process_survives() {
    let fx = WorkspaceFixture::empty();
    let exec = MontyExecutor::new();
    let ctx = tool_ctx(fx.root(), CancellationToken::new());
    let start = std::time::Instant::now();
    let out = exec
        .run(&ctx, "while True:\n    pass", 500)
        .await
        .expect("run returns Ok on timeout");
    let elapsed = start.elapsed();
    assert!(out.timed_out, "infinite loop should mark timed_out");
    assert_ne!(out.exit_code, 0);
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "timeout should fire well before 3s, took {elapsed:?}"
    );
}

// ---------------------------------------------------------------------------
// Curated env — secrets never leak.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn secret_env_var_is_not_visible() {
    // `OPENAI_API_KEY` is not on the curated allowlist, so the guest reads
    // the caller's default (None) regardless of the host environment — the
    // dispatch never even calls `std::env::var` for a non-allowlisted key.
    // A bare `None` last-expression is suppressed from stdout, so success is
    // an empty echo (and, critically, no secret value in the output).
    let out = run_in(&[], "import os\nos.getenv('OPENAI_API_KEY', 'fallback')").await;
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    // The default we passed IS returned (proving the var is treated as unset),
    // but no real secret value can appear.
    assert_eq!(
        out.stdout.trim(),
        "fallback",
        "non-allowlisted var must read as unset (default returned)"
    );
}

// ---------------------------------------------------------------------------
// Error shaping — a guest exception is stderr + non-zero exit, not a crash.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn guest_exception_becomes_stderr_and_nonzero_exit() {
    let out = run_in(&[], "1 / 0").await;
    assert_ne!(out.exit_code, 0);
    assert!(
        out.stderr.contains("ZeroDivisionError"),
        "expected ZeroDivisionError, got: {}",
        out.stderr
    );
}

#[tokio::test]
async fn syntax_error_becomes_stderr_not_panic() {
    let out = run_in(&[], "def broken(:\n    pass").await;
    assert_ne!(out.exit_code, 0);
    assert!(!out.stderr.is_empty(), "expected a parse error on stderr");
}

// ---------------------------------------------------------------------------
// Unsupported-syntax fidelity. The plan listed `class`/`match` as out-of-scope,
// but the PINNED Monty rev actually implements basic classes — this suite pins
// what the shipped interpreter really does so the contract is observable, not
// assumed:
//   - basic `class` (fields, `__init__`, methods) WORKS — must not crash;
//   - class INHERITANCE (`class B(A)`) is NOT supported — must fail LOUDLY;
//   - `match`/`case` is NOT supported — must fail LOUDLY.
// "Fail loudly" = non-zero exit + a traceback on stderr, never a silent
// mis-execute or a host panic (the executor still returns Ok(PythonOutput)).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn basic_class_is_supported_by_pinned_monty() {
    // Contradicts the plan's stale "class out of scope" note: the pinned rev
    // runs a simple class with a method + instance attribute. Pin it so a
    // future Monty bump that regresses this is caught.
    let out = run_in(
        &[],
        "class Counter:\n    def __init__(self):\n        self.n = 0\n    def bump(self):\n        self.n += 1\n        return self.n\nc = Counter()\nc.bump()\nprint(c.bump())",
    )
    .await;
    assert_eq!(
        out.exit_code, 0,
        "basic class should run; stderr: {}",
        out.stderr
    );
    assert_eq!(out.stdout.trim(), "2");
}

#[tokio::test]
async fn class_inheritance_fails_loud_not_silent() {
    let out = run_in(&[], "class A:\n    pass\nclass B(A):\n    pass\nB()").await;
    assert_ne!(
        out.exit_code, 0,
        "unsupported class inheritance must not silently succeed"
    );
    assert!(
        !out.stderr.is_empty(),
        "unsupported inheritance must surface an error on stderr, got empty"
    );
}

#[tokio::test]
async fn match_statement_fails_loud_not_silent() {
    let out = run_in(&[], "x = 1\nmatch x:\n    case 1:\n        print('one')\n").await;
    assert_ne!(out.exit_code, 0, "match stmt must not silently succeed");
    assert!(
        !out.stderr.is_empty(),
        "unsupported `match` must surface an error on stderr, got empty"
    );
}
