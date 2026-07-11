//! `EmulatedShellExecutor` integration — proves the Phase 2 success
//! criteria: the in-process interpreter runs pipelines / redirects / loops
//! / conditionals / vars / globs entirely through `ctx.fs`, blocks binary
//! execution by construction, and folds filesystem errors into a non-zero
//! exit + stderr instead of crashing the tool call.

mod common;

use std::path::Path;

use common::tempdir_workspace::WorkspaceFixture;
use common::tool_ctx_harness::tool_ctx;
use openlet_adapters::emushell::EmulatedShellExecutor;
use openlet_core::tools::builtins::bash::{BashOutput, ShellExecutor};
use tokio_util::sync::CancellationToken;

/// Run `command` against a workspace seeded with `files`.
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

#[tokio::test]
async fn echo_prints_to_stdout() {
    let out = run_in(&[], "echo hello world").await;
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "hello world\n");
}

#[tokio::test]
async fn pipeline_feeds_stdout_into_stdin() {
    // `cat` with no args echoes its stdin; chain two to prove piping.
    let out = run_in(&[("a.txt", "from-file\n")], "cat a.txt | cat").await;
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "from-file\n");
}

#[tokio::test]
async fn cat_reads_through_fs() {
    let out = run_in(&[("notes/todo.txt", "buy milk\n")], "cat notes/todo.txt").await;
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "buy milk\n");
}

#[tokio::test]
async fn redirect_truncate_writes_file() {
    let fx = WorkspaceFixture::empty();
    let out = run_at(fx.root(), "echo written > out.txt").await;
    assert_eq!(out.exit_code, 0);
    // Redirected stdout does not surface.
    assert_eq!(out.stdout, "");
    let body = std::fs::read_to_string(fx.root().join("out.txt")).unwrap();
    assert_eq!(body, "written\n");
}

#[tokio::test]
async fn redirect_append_adds_to_file() {
    let fx = WorkspaceFixture::with_files([("log.txt", "line1\n")]);
    let out = run_at(fx.root(), "echo line2 >> log.txt").await;
    assert_eq!(out.exit_code, 0);
    let body = std::fs::read_to_string(fx.root().join("log.txt")).unwrap();
    assert_eq!(body, "line1\nline2\n");
}

#[tokio::test]
async fn input_redirect_reads_file() {
    let out = run_in(&[("in.txt", "piped-in\n")], "cat < in.txt").await;
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "piped-in\n");
}

#[tokio::test]
async fn glob_expands_via_fs_not_host() {
    // Two matching files + one non-matching. `echo *.txt` should list only
    // the .txt names, sorted, and never see host-cwd files.
    let out = run_in(
        &[("a.txt", "x"), ("b.txt", "y"), ("c.md", "z")],
        "echo *.txt",
    )
    .await;
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "a.txt b.txt\n");
}

#[tokio::test]
async fn glob_no_match_passes_literal() {
    let out = run_in(&[("a.txt", "x")], "echo *.none").await;
    assert_eq!(out.exit_code, 0);
    // nullglob off (bash default): unmatched pattern stays literal.
    assert_eq!(out.stdout, "*.none\n");
}

#[tokio::test]
async fn and_or_short_circuit() {
    // `false || echo b` runs the echo; `true && echo a` runs it too.
    let out = run_in(&[], "true && echo a").await;
    assert_eq!(out.stdout, "a\n");

    let out = run_in(&[], "false || echo b").await;
    assert_eq!(out.stdout, "b\n");

    // `false && echo skipped` must NOT run the echo.
    let out = run_in(&[], "false && echo skipped").await;
    assert_eq!(out.stdout, "");
    assert_ne!(out.exit_code, 0);
}

#[tokio::test]
async fn for_loop_iterates_glob() {
    let out = run_in(
        &[("a.txt", "AA"), ("b.txt", "BB")],
        "for f in *.txt; do cat $f; done",
    )
    .await;
    assert_eq!(out.exit_code, 0);
    // Concatenated file bodies in sorted glob order.
    assert_eq!(out.stdout, "AABB");
}

#[tokio::test]
async fn variable_assignment_and_expansion() {
    let out = run_in(&[], "x=hi; echo $x").await;
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "hi\n");
}

#[tokio::test]
async fn command_substitution_reenters_eval() {
    let out = run_in(&[("name.txt", "world\n")], "echo hello $(cat name.txt)").await;
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "hello world\n");
}

#[tokio::test]
async fn if_then_runs_on_success() {
    let out = run_in(&[], "if true; then echo yes; fi").await;
    assert_eq!(out.stdout, "yes\n");
}

#[tokio::test]
async fn if_else_runs_on_failure() {
    let out = run_in(&[], "if false; then echo yes; else echo no; fi").await;
    assert_eq!(out.stdout, "no\n");
}

// --- security by construction ------------------------------------------

#[tokio::test]
async fn unknown_binary_is_command_not_found() {
    let out = run_in(&[], "cargo build").await;
    assert_eq!(out.exit_code, 127);
    assert!(
        out.stderr.contains("command not found"),
        "stderr: {}",
        out.stderr
    );
    assert_eq!(out.stdout, "");
}

#[tokio::test]
async fn curl_blocked_by_construction() {
    let out = run_in(&[], "curl http://example.com").await;
    assert_eq!(out.exit_code, 127);
    assert!(out.stderr.contains("command not found"));
}

#[tokio::test]
async fn dev_tcp_redirect_has_no_network_by_construction() {
    // In real bash, `> /dev/tcp/host/port` opens a socket — a network egress
    // channel. The interpreter has no code that opens a socket and no special
    // `/dev/tcp` handling, so the redirect target is treated as an ordinary
    // workspace-relative path. The FS boundary rejects the absolute `/dev/...`
    // path (OutsideWorkspace), surfacing as a non-zero exit with NOTHING sent
    // over any network. The security property is structural: there is simply
    // no socket syscall to reach.
    let out = run_in(&[], "echo pwned > /dev/tcp/127.0.0.1/9999").await;
    assert_ne!(
        out.exit_code, 0,
        "writing to /dev/tcp must fail, not open a socket"
    );
    assert_eq!(out.stdout, "", "no data should surface on stdout");
}

#[tokio::test]
async fn escape_outside_workspace_is_denied() {
    // `cat ../secret` must be rejected by the Filesystem boundary, surfacing
    // as a non-zero exit + stderr — NOT a host read and NOT a tool crash.
    let out = run_in(&[("inside.txt", "ok")], "cat ../../../etc/passwd").await;
    assert_ne!(out.exit_code, 0);
    assert!(
        !out.stdout.contains("root:"),
        "must not read host /etc/passwd"
    );
}

#[tokio::test]
async fn fs_error_folds_into_exit_code_not_toolerror() {
    // `cat missing && echo x`: the missing file makes cat exit non-zero, so
    // the `&&` short-circuits and `x` is never printed. Critically, run()
    // still returns Ok(BashOutput) — the FsError does not escape as
    // Err(ToolError) (red-team FM5).
    let out = run_in(&[], "cat missing.txt && echo x").await;
    assert_ne!(out.exit_code, 0);
    assert_eq!(out.stdout, "");
    assert!(
        out.stderr.contains("No such file"),
        "stderr: {}",
        out.stderr
    );
}

#[tokio::test]
async fn stdout_cap_truncates_on_char_boundary() {
    // Cat a file whose contents exceed the 256 KiB stdout cap and are made
    // entirely of a 3-byte UTF-8 char (`√`). A naive byte-slice at the cap
    // would land mid-character and panic; the interpreter must floor to a
    // char boundary and set the truncated flag instead of crashing.
    let big = "√".repeat(120_000); // 360 KB > 256 KiB cap
    let out = run_in(&[("big.txt", big.as_str())], "cat big.txt").await;
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout_truncated, "expected truncated flag");
    // The surviving prefix must still be valid UTF-8 (String guarantees it;
    // the real assertion is that we got here without a panic).
    assert!(out.stdout.len() <= 256 * 1024);
    assert!(out.stdout.chars().all(|c| c == '√'));
}
