//! `python` / `python3` inside the emulated bash tool.
//!
//! Phase 7 follow-up: an LLM that reflexively types `python3 script.py` in the
//! `bash` tool should get a real in-process run (via the SAME Monty interpreter
//! the standalone `python` tool uses) instead of `command not found`. These
//! tests pin the supported invocation forms + the Monty limitations that are
//! surfaced loudly rather than silently mis-executed.

mod common;

use std::path::Path;

use common::tempdir_workspace::WorkspaceFixture;
use common::tool_ctx_harness::tool_ctx;
use openlet_adapters::emushell::EmulatedShellExecutor;
use openlet_core::tools::builtins::bash::{BashOutput, ShellExecutor};
use tokio_util::sync::CancellationToken;

async fn run_in(files: &[(&str, &str)], command: &str) -> BashOutput {
    let owned: Vec<(String, String)> = files
        .iter()
        .map(|(p, c)| ((*p).to_string(), (*c).to_string()))
        .collect();
    let fx = WorkspaceFixture::with_files(owned.iter().map(|(p, c)| (p.as_str(), c.as_str())));
    run_at(fx.root(), command).await
}

async fn run_at(root: &Path, command: &str) -> BashOutput {
    let exec = EmulatedShellExecutor::new();
    let ctx = tool_ctx(root, CancellationToken::new());
    exec.run(&ctx, command, 5_000)
        .await
        .expect("interpreter run should return Ok(BashOutput), never Err")
}

// --- supported forms ---------------------------------------------------

#[tokio::test]
async fn python3_is_no_longer_command_not_found() {
    // The whole point: the pre-cutover behavior was exit 127. Now it runs.
    let out = run_in(&[], "python3 -c 'print(1+1)'").await;
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert!(!out.stderr.contains("command not found"));
    assert_eq!(out.stdout, "2\n");
}

#[tokio::test]
async fn python_dash_c_runs_inline_code() {
    let out = run_in(&[], "python -c 'print(\"hi\")'").await;
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert_eq!(out.stdout, "hi\n");
}

#[tokio::test]
async fn python_runs_a_script_file_through_fs() {
    let out = run_in(
        &[("compute.py", "print(sum(range(5)))\n")],
        "python3 compute.py",
    )
    .await;
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert_eq!(out.stdout, "10\n");
}

#[tokio::test]
async fn python_reads_program_from_stdin_when_no_operand() {
    // Real python3 reads a program from stdin when given neither -c nor a file.
    let out = run_in(&[], "echo 'print(6*7)' | python3").await;
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert_eq!(out.stdout, "42\n");
}

#[tokio::test]
async fn python_output_pipes_into_next_stage() {
    // The builtin's stdout must feed a downstream pipe like any other builtin.
    let out = run_in(&[], "python3 -c 'print(\"b\"); print(\"a\")' | sort").await;
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    assert_eq!(out.stdout, "a\nb\n");
}

#[tokio::test]
async fn python_script_writes_through_fs() {
    // A script run via the bash builtin persists through ctx.fs, same seam as
    // the standalone python tool.
    let fx =
        WorkspaceFixture::with_files([("w.py", "open('out.txt','w').write('via-bash-python')\n")]);
    let out = run_at(fx.root(), "python3 w.py").await;
    assert_eq!(out.exit_code, 0, "stderr: {}", out.stderr);
    let disk = std::fs::read_to_string(fx.root().join("out.txt")).unwrap();
    assert_eq!(disk, "via-bash-python");
}

// --- deny-by-construction carries over ---------------------------------

#[tokio::test]
async fn python_in_bash_still_denies_subprocess() {
    // Routing through Monty means the sandbox holds: no os.system escape even
    // when reached via the bash tool.
    let out = run_in(&[], "python3 -c 'import os; os.system(\"id\")'").await;
    assert_ne!(
        out.exit_code, 0,
        "os.system must not succeed via bash python"
    );
}

// --- Monty limitations surfaced loudly (never silently wrong) ----------

#[tokio::test]
async fn sys_argv_fails_loud_not_silent() {
    // Monty's `sys` has no `argv`; a script that reads it raises a real
    // AttributeError rather than seeing a wrong/empty argv. Extra operands are
    // accepted at the shell level but the program that depends on them errors.
    let out = run_in(
        &[("a.py", "import sys\nprint(sys.argv[1])\n")],
        "python3 a.py first",
    )
    .await;
    assert_ne!(out.exit_code, 0, "sys.argv access must fail loud");
    assert!(
        out.stderr.contains("argv") || out.stderr.contains("AttributeError"),
        "expected an argv-related error, got: {}",
        out.stderr
    );
}

#[tokio::test]
async fn unsupported_module_flag_fails_loud() {
    let out = run_in(&[], "python3 -m http.server").await;
    assert_ne!(out.exit_code, 0);
    assert!(out.stderr.contains("-m"), "stderr: {}", out.stderr);
}

#[tokio::test]
async fn dash_c_without_argument_errors() {
    let out = run_in(&[], "python3 -c").await;
    assert_eq!(out.exit_code, 2);
    assert!(out.stderr.contains("-c"), "stderr: {}", out.stderr);
}

#[tokio::test]
async fn missing_script_file_reports_not_found() {
    let out = run_in(&[], "python3 nope.py").await;
    assert_ne!(out.exit_code, 0);
    assert!(
        out.stderr.contains("No such file") || out.stderr.contains("nope.py"),
        "stderr: {}",
        out.stderr
    );
}
