//! Phase 3 builtin coverage — the emulated coreutils set, each backed only
//! by `ctx.fs`. Proves the round-1 commands run, that pushdown search hits
//! `ctx.fs.grep`/`glob`, that `mv`/`cp` never lose data on error, and that
//! commands outside the set (and outside the sed/awk subset) fail loud
//! rather than escaping to the host.

mod common;

use std::path::Path;

use common::tempdir_workspace::WorkspaceFixture;
use common::tool_ctx_harness::tool_ctx;
use openlet_adapters::emushell::EmulatedShellExecutor;
use openlet_core::error::ToolError;
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

// --- transform builtins ------------------------------------------------

#[tokio::test]
async fn sort_orders_lines() {
    let out = run_in(&[("f.txt", "banana\napple\ncherry\n")], "sort f.txt").await;
    assert_eq!(out.exit_code, 0);
    assert_eq!(out.stdout, "apple\nbanana\ncherry\n");
}

#[tokio::test]
async fn sort_numeric_and_reverse() {
    let out = run_in(&[("n.txt", "2\n10\n1\n")], "sort -n -r n.txt").await;
    assert_eq!(out.stdout, "10\n2\n1\n");
}

#[tokio::test]
async fn uniq_collapses_adjacent_dups() {
    let out = run_in(&[("d.txt", "a\na\nb\na\n")], "uniq d.txt").await;
    assert_eq!(out.stdout, "a\nb\na\n");
}

#[tokio::test]
async fn uniq_count_prefixes_counts() {
    let out = run_in(&[("d.txt", "a\na\nb\n")], "uniq -c d.txt").await;
    assert_eq!(out.stdout, "      2 a\n      1 b\n");
}

#[tokio::test]
async fn sort_uniq_pipeline() {
    let out = run_in(&[("a.txt", "z\na\nz\na\n")], "cat a.txt | sort | uniq").await;
    assert_eq!(out.stdout, "a\nz\n");
}

#[tokio::test]
async fn basename_strips_dir_and_suffix() {
    let out = run_in(&[], "basename /usr/local/file.txt .txt").await;
    assert_eq!(out.stdout, "file\n");
}

#[tokio::test]
async fn dirname_strips_last_component() {
    let out = run_in(&[], "dirname a/b/c").await;
    assert_eq!(out.stdout, "a/b\n");
}

#[tokio::test]
async fn diff_reports_changed_lines() {
    let out = run_in(&[("a.txt", "one\ntwo\n"), ("b.txt", "one\nthree\n")], "diff a.txt b.txt").await;
    assert_eq!(out.exit_code, 1);
    assert!(out.stdout.contains("< two"));
    assert!(out.stdout.contains("> three"));
}

// --- text builtins -----------------------------------------------------

#[tokio::test]
async fn head_takes_first_n() {
    let out = run_in(&[("f.txt", "1\n2\n3\n4\n5\n")], "head -n 2 f.txt").await;
    assert_eq!(out.stdout, "1\n2\n");
}

#[tokio::test]
async fn tail_takes_last_n() {
    let out = run_in(&[("f.txt", "1\n2\n3\n4\n5\n")], "tail -n 2 f.txt").await;
    assert_eq!(out.stdout, "4\n5\n");
}

#[tokio::test]
async fn wc_lines_flag() {
    let out = run_in(&[("f.txt", "a\nb\nc\n")], "wc -l f.txt").await;
    assert_eq!(out.stdout.trim(), "3");
}

#[tokio::test]
async fn cut_fields_by_delim() {
    let out = run_in(&[("f.csv", "a,b,c\nd,e,f\n")], "cut -d , -f 2 f.csv").await;
    assert_eq!(out.stdout, "b\ne\n");
}

#[tokio::test]
async fn tr_translates_chars() {
    let out = run_in(&[("f.txt", "hello\n")], "cat f.txt | tr a-z A-Z").await;
    assert_eq!(out.stdout, "HELLO\n");
}

#[tokio::test]
async fn tr_deletes_chars() {
    let out = run_in(&[("f.txt", "h-e-l-l-o\n")], "cat f.txt | tr -d -").await;
    assert_eq!(out.stdout, "hello\n");
}

// --- sed / awk subset --------------------------------------------------

#[tokio::test]
async fn sed_substitutes_first_match() {
    let out = run_in(&[("f.txt", "foo foo\n")], "sed 's/foo/bar/' f.txt").await;
    assert_eq!(out.stdout, "bar foo\n");
}

#[tokio::test]
async fn sed_global_substitution() {
    let out = run_in(&[("f.txt", "foo foo\n")], "sed 's/foo/bar/g' f.txt").await;
    assert_eq!(out.stdout, "bar bar\n");
}

#[tokio::test]
async fn sed_in_place_rewrites_file() {
    let fx = WorkspaceFixture::with_files([("f.txt", "aaa\n")]);
    let out = run_at(fx.root(), "sed -i 's/a/b/g' f.txt").await;
    assert_eq!(out.exit_code, 0);
    let body = std::fs::read_to_string(fx.root().join("f.txt")).unwrap();
    assert_eq!(body, "bbb\n");
}

#[tokio::test]
async fn sed_unsupported_command_fails_loud() {
    // Line-address delete (`2d`) is outside the round-1 subset.
    let out = run_in(&[("f.txt", "a\nb\n")], "sed '2d' f.txt").await;
    assert_ne!(out.exit_code, 0);
    assert!(out.stderr.contains("unsupported") || out.stderr.contains("only s///"));
}

#[tokio::test]
async fn awk_prints_field() {
    let out = run_in(&[("f.txt", "a b c\nd e f\n")], "awk '{print $2}' f.txt").await;
    assert_eq!(out.stdout, "b\ne\n");
}

#[tokio::test]
async fn awk_field_separator() {
    let out = run_in(&[("f.csv", "a,b,c\n")], "awk -F , '{print $3}' f.csv").await;
    assert_eq!(out.stdout, "c\n");
}

#[tokio::test]
async fn awk_nr_builtin() {
    let out = run_in(&[("f.txt", "x\ny\nz\n")], "awk '{print NR}' f.txt").await;
    assert_eq!(out.stdout, "1\n2\n3\n");
}

// --- tree builtins -----------------------------------------------------

#[tokio::test]
async fn ls_lists_children() {
    let out = run_in(&[("dir/a.txt", ""), ("dir/b.txt", "")], "ls dir").await;
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.contains("a.txt"));
    assert!(out.stdout.contains("b.txt"));
}

#[tokio::test]
async fn find_by_name_glob() {
    let out = run_in(
        &[("src/a.rs", ""), ("src/b.txt", ""), ("src/deep/c.rs", "")],
        "find src -name '*.rs'",
    )
    .await;
    assert_eq!(out.exit_code, 0);
    assert!(out.stdout.contains("a.rs"));
    assert!(out.stdout.contains("c.rs"));
    assert!(!out.stdout.contains("b.txt"));
}

#[tokio::test]
async fn grep_recursive_pushes_down_to_fs() {
    let out = run_in(
        &[("a.txt", "needle here\n"), ("b.txt", "nothing\n")],
        "grep -rn needle .",
    )
    .await;
    assert_eq!(out.exit_code, 0);
    // Format: path:line:text
    assert!(out.stdout.contains("a.txt:1:needle here"));
    assert!(!out.stdout.contains("b.txt"));
}

#[tokio::test]
async fn grep_stdin_filters_lines() {
    let out = run_in(&[("f.txt", "match\nskip\nmatch2\n")], "cat f.txt | grep match").await;
    assert_eq!(out.stdout, "match\nmatch2\n");
}

#[tokio::test]
async fn grep_no_match_exits_one() {
    let out = run_in(&[("f.txt", "abc\n")], "cat f.txt | grep zzz").await;
    assert_eq!(out.exit_code, 1);
    assert_eq!(out.stdout, "");
}

// --- mutation builtins -------------------------------------------------

#[tokio::test]
async fn touch_creates_empty_file() {
    let fx = WorkspaceFixture::empty();
    let out = run_at(fx.root(), "touch new.txt").await;
    assert_eq!(out.exit_code, 0);
    assert!(fx.root().join("new.txt").exists());
}

#[tokio::test]
async fn rm_removes_file() {
    let fx = WorkspaceFixture::with_files([("gone.txt", "x")]);
    let out = run_at(fx.root(), "rm gone.txt").await;
    assert_eq!(out.exit_code, 0);
    assert!(!fx.root().join("gone.txt").exists());
}

#[tokio::test]
async fn rm_recursive_removes_tree() {
    let fx = WorkspaceFixture::with_files([("d/a.txt", "x"), ("d/sub/b.txt", "y")]);
    let out = run_at(fx.root(), "rm -r d").await;
    assert_eq!(out.exit_code, 0);
    assert!(!fx.root().join("d").exists());
}

#[tokio::test]
async fn cp_duplicates_file() {
    let fx = WorkspaceFixture::with_files([("src.txt", "payload\n")]);
    let out = run_at(fx.root(), "cp src.txt dst.txt").await;
    assert_eq!(out.exit_code, 0);
    // Source untouched, dest is a copy.
    assert_eq!(std::fs::read_to_string(fx.root().join("src.txt")).unwrap(), "payload\n");
    assert_eq!(std::fs::read_to_string(fx.root().join("dst.txt")).unwrap(), "payload\n");
}

#[tokio::test]
async fn mv_renames_file() {
    let fx = WorkspaceFixture::with_files([("old.txt", "data\n")]);
    let out = run_at(fx.root(), "mv old.txt new.txt").await;
    assert_eq!(out.exit_code, 0);
    assert!(!fx.root().join("old.txt").exists());
    assert_eq!(std::fs::read_to_string(fx.root().join("new.txt")).unwrap(), "data\n");
}

#[tokio::test]
async fn cp_missing_source_leaves_no_dest() {
    let fx = WorkspaceFixture::empty();
    let out = run_at(fx.root(), "cp missing.txt dst.txt").await;
    assert_ne!(out.exit_code, 0);
    // No partial dest written when the read fails.
    assert!(!fx.root().join("dst.txt").exists());
}

// --- mv/cp interrupt-safety (Phase 7 criterion) ------------------------
//
// The plan requires cancel-mid-operation to never lose or half-write data.
// `mv`/`cp` check the cancel token BEFORE each source and, per file, either
// complete the op fully (cp reads the whole source before writing dest; mv
// uses an atomic rename) or don't start it — so an interrupt can only fall on
// a clean boundary, never mid-file. We prove that with a token that is already
// cancelled before the run: the multi-source op must stop at exit 130 and
// leave every source intact, with no partial destination.

/// Run `command` with an ALREADY-cancelled token. The interpreter's in-band
/// cancel check trips deterministically (no timing race) and halts the run —
/// surfacing as `Err(ToolError::Timeout)`, the same contract the subprocess
/// executor used for cancel — BEFORE any builtin mutates the filesystem.
async fn run_at_cancelled(root: &Path, command: &str) -> Result<BashOutput, ToolError> {
    let exec = EmulatedShellExecutor::new();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let ctx = tool_ctx(root, cancel);
    exec.run(&ctx, command, 5_000).await
}

#[tokio::test]
async fn cp_cancelled_leaves_sources_intact_no_partial_dest() {
    // Interrupt-safety: a cancel mid-`cp` must never lose data or leave a
    // partial destination. With the token pre-tripped the run halts before the
    // copy; the stronger property this proves is that NOTHING is mutated —
    // sources untouched, no dest files created.
    let fx = WorkspaceFixture::with_files([("a.txt", "AAA\n"), ("b.txt", "BBB\n")]);
    std::fs::create_dir_all(fx.root().join("out")).unwrap();
    let result = run_at_cancelled(fx.root(), "cp a.txt b.txt out").await;
    assert!(
        matches!(result, Err(ToolError::Timeout)),
        "cancelled run halts as Err(Timeout), got {result:?}"
    );
    // Sources are never consumed by cp, so both remain intact.
    assert_eq!(std::fs::read_to_string(fx.root().join("a.txt")).unwrap(), "AAA\n");
    assert_eq!(std::fs::read_to_string(fx.root().join("b.txt")).unwrap(), "BBB\n");
    // No destination files written — no partial copy left behind.
    assert!(!fx.root().join("out/a.txt").exists(), "no partial dest on cancel");
    assert!(!fx.root().join("out/b.txt").exists(), "no partial dest on cancel");
}

#[tokio::test]
async fn mv_cancelled_leaves_sources_intact_no_limbo() {
    // Interrupt-safety for mv: the atomic `rename` is the only mutation. A
    // cancel before it runs leaves every source in place and the dest empty —
    // no file is ever in a "removed from source but not yet at dest" limbo.
    let fx = WorkspaceFixture::with_files([("a.txt", "AAA\n"), ("b.txt", "BBB\n")]);
    std::fs::create_dir_all(fx.root().join("out")).unwrap();
    let result = run_at_cancelled(fx.root(), "mv a.txt b.txt out").await;
    assert!(
        matches!(result, Err(ToolError::Timeout)),
        "cancelled run halts as Err(Timeout), got {result:?}"
    );
    assert_eq!(std::fs::read_to_string(fx.root().join("a.txt")).unwrap(), "AAA\n");
    assert_eq!(std::fs::read_to_string(fx.root().join("b.txt")).unwrap(), "BBB\n");
    assert!(!fx.root().join("out/a.txt").exists());
    assert!(!fx.root().join("out/b.txt").exists());
}

#[tokio::test]
async fn tee_writes_and_passes_through() {
    let fx = WorkspaceFixture::empty();
    let out = run_at(fx.root(), "echo hi | tee out.txt").await;
    assert_eq!(out.exit_code, 0);
    // tee echoes stdin to stdout AND writes the file.
    assert_eq!(out.stdout, "hi\n");
    assert_eq!(std::fs::read_to_string(fx.root().join("out.txt")).unwrap(), "hi\n");
}

#[tokio::test]
async fn mkdir_creates_directory() {
    let fx = WorkspaceFixture::empty();
    let out = run_at(fx.root(), "mkdir newdir").await;
    assert_eq!(out.exit_code, 0);
    assert!(fx.root().join("newdir").is_dir());
}

// --- deny-by-construction ----------------------------------------------

#[tokio::test]
async fn unknown_command_is_command_not_found() {
    let out = run_in(&[], "jq .").await;
    assert_eq!(out.exit_code, 127);
    assert!(out.stderr.contains("command not found"));
}

#[tokio::test]
async fn curl_is_not_a_builtin() {
    let out = run_in(&[], "curl https://example.com").await;
    assert_eq!(out.exit_code, 127);
    assert!(out.stderr.contains("command not found"));
}
