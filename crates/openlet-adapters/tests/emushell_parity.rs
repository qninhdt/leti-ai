//! FS-impl parity — the load-bearing proof of this plan's thesis.
//!
//! The emulated bash + python interpreters hold nothing but
//! `Arc<dyn Filesystem>`. Local vs cloud differ ONLY in which impl is injected;
//! the interpreters are byte-identical. This suite runs the SAME script through
//! the SAME executor twice — once over `LocalFilesystem` (real `tokio::fs` on a
//! tempdir) and once over `MemFilesystem` (a structurally-unrelated in-memory
//! `HashMap` store) — and asserts stdout + exit match. A dedicated test also
//! reads the post-run state back through each FS seam to prove mutations
//! (`write`/`mv`) land identically.
//!
//! If they match, the interpreter's behavior is a function of the `Filesystem`
//! seam alone, not of incidental disk behavior — which is exactly the property
//! that lets a cloud gRPC FS (Phase 6) drop in without touching the executors.
//! (A real cloud backend can't run under `cargo test`; that's Phase 6's gated
//! live e2e. This mock is the stand-in second impl, with glob/grep sharing the
//! very `globset`/`regex` crates `LocalFilesystem` uses so the dialect is
//! identical by construction.)

mod common;

use std::sync::Arc;

use common::mem_fs::MemFilesystem;
use common::tempdir_workspace::WorkspaceFixture;
use common::tool_ctx_harness::tool_ctx_with_fs;
use openlet_adapters::emushell::EmulatedShellExecutor;
use openlet_adapters::localfs::LocalFilesystem;
use openlet_adapters::pyexec::MontyExecutor;
use openlet_core::adapters::Filesystem;
use openlet_core::tools::builtins::bash::{BashOutput, ShellExecutor};
use openlet_core::tools::builtins::python::{PythonExecutor, PythonOutput};
use tokio_util::sync::CancellationToken;

/// Run `command` under the emulated bash executor against an arbitrary FS impl.
async fn bash_on(fs: Arc<dyn Filesystem>, command: &str) -> BashOutput {
    let exec = EmulatedShellExecutor::new();
    let ctx = tool_ctx_with_fs(fs, CancellationToken::new());
    exec.run(&ctx, command, 5_000)
        .await
        .expect("bash run should return Ok(BashOutput)")
}

/// Run `code` under the Monty python executor against an arbitrary FS impl.
async fn python_on(fs: Arc<dyn Filesystem>, code: &str) -> PythonOutput {
    let exec = MontyExecutor::new();
    let ctx = tool_ctx_with_fs(fs, CancellationToken::new());
    exec.run(&ctx, code, 5_000)
        .await
        .expect("python run should return Ok(PythonOutput)")
}

/// Assert a bash script produces identical stdout + exit on both impls, seeded
/// with the same files. Returns the (shared) stdout for further assertions.
async fn assert_bash_parity(files: &[(&str, &str)], command: &str) -> BashOutput {
    let fx = WorkspaceFixture::with_files(files.iter().copied());
    let local: Arc<dyn Filesystem> = Arc::new(LocalFilesystem::new(fx.root().to_path_buf()));
    let mem: Arc<dyn Filesystem> = Arc::new(MemFilesystem::seed(files.iter().copied()));

    let local_out = bash_on(local, command).await;
    let mem_out = bash_on(mem, command).await;

    assert_eq!(
        local_out.stdout, mem_out.stdout,
        "bash stdout diverged for `{command}`\nlocal: {:?}\nmem:   {:?}",
        local_out.stdout, mem_out.stdout
    );
    assert_eq!(
        local_out.exit_code, mem_out.exit_code,
        "bash exit_code diverged for `{command}`"
    );
    local_out
}

/// Same, for the python executor.
async fn assert_python_parity(files: &[(&str, &str)], code: &str) -> PythonOutput {
    let fx = WorkspaceFixture::with_files(files.iter().copied());
    let local: Arc<dyn Filesystem> = Arc::new(LocalFilesystem::new(fx.root().to_path_buf()));
    let mem: Arc<dyn Filesystem> = Arc::new(MemFilesystem::seed(files.iter().copied()));

    let local_out = python_on(local, code).await;
    let mem_out = python_on(mem, code).await;

    assert_eq!(
        local_out.stdout, mem_out.stdout,
        "python stdout diverged for code:\n{code}\nlocal: {:?}\nmem:   {:?}",
        local_out.stdout, mem_out.stdout
    );
    assert_eq!(
        local_out.exit_code, mem_out.exit_code,
        "python exit_code diverged for code:\n{code}"
    );
    local_out
}

// --- bash parity -------------------------------------------------------

#[tokio::test]
async fn bash_cat_parity() {
    let out = assert_bash_parity(&[("a.txt", "hello\nworld\n")], "cat a.txt").await;
    assert_eq!(out.stdout, "hello\nworld\n");
}

#[tokio::test]
async fn bash_pipeline_sort_uniq_parity() {
    let out = assert_bash_parity(
        &[("f.txt", "z\na\nz\na\nb\n")],
        "cat f.txt | sort | uniq",
    )
    .await;
    assert_eq!(out.stdout, "a\nb\nz\n");
}

#[tokio::test]
async fn bash_glob_expansion_parity() {
    // Word-expansion globs request PathAsc, so both impls order identically.
    let out = assert_bash_parity(
        &[("a.txt", "x"), ("b.txt", "y"), ("c.md", "z")],
        "echo *.txt",
    )
    .await;
    assert_eq!(out.stdout, "a.txt b.txt\n");
}

#[tokio::test]
async fn bash_for_loop_over_glob_parity() {
    let out = assert_bash_parity(
        &[("a.txt", "AA"), ("b.txt", "BB")],
        "for f in *.txt; do cat $f; done",
    )
    .await;
    assert_eq!(out.stdout, "AABB");
}

#[tokio::test]
async fn bash_grep_recursive_parity() {
    // Single matching file keeps multi-file walk-order out of the picture.
    let out = assert_bash_parity(
        &[("a.txt", "needle here\n"), ("b.txt", "nothing\n")],
        "grep -rn needle .",
    )
    .await;
    assert!(out.stdout.contains("a.txt:1:needle here"));
}

#[tokio::test]
async fn bash_grep_multifile_sorted_parity() {
    // Multi-file grep walk order is unspecified on disk, so pipe through sort
    // to compare a canonical ordering across both impls.
    let out = assert_bash_parity(
        &[("a.txt", "hit\n"), ("b.txt", "hit\n"), ("c.txt", "miss\n")],
        "grep -rl hit . | sort",
    )
    .await;
    assert_eq!(out.exit_code, 0);
}

#[tokio::test]
async fn bash_command_not_found_parity() {
    let out = assert_bash_parity(&[], "cargo build").await;
    assert_eq!(out.exit_code, 127);
}

#[tokio::test]
async fn bash_missing_file_error_parity() {
    // The FsError→stderr+exit folding must be identical: a missing file makes
    // `cat` non-zero on both impls, short-circuiting the `&&`.
    let out = assert_bash_parity(&[], "cat missing.txt && echo x").await;
    assert_ne!(out.exit_code, 0);
    assert_eq!(out.stdout, "");
}

// --- python parity -----------------------------------------------------

#[tokio::test]
async fn python_read_parity() {
    let out = assert_python_parity(&[("in.txt", "payload")], "open('in.txt').read()").await;
    assert_eq!(out.stdout.trim_end(), "payload");
}

#[tokio::test]
async fn python_computation_parity() {
    // Pure computation is FS-independent, but proves the executor wiring
    // itself is identical across both contexts.
    assert_python_parity(&[], "sum(i*i for i in range(10))").await;
}

#[tokio::test]
async fn python_write_readback_parity() {
    // A write then read within the SAME run must observe the write on both
    // impls (proves the FS seam is the single source of truth, not disk).
    let out = assert_python_parity(
        &[],
        "open('out.txt', 'w').write('abc')\nopen('out.txt').read()",
    )
    .await;
    assert_eq!(out.stdout.trim_end(), "abc");
}

#[tokio::test]
async fn python_escape_denied_parity() {
    // Both impls must reject a parent-traversal write with the same failure.
    let out = assert_python_parity(&[], "open('../escape.txt', 'w').write('x')").await;
    assert_ne!(out.exit_code, 0);
}

// --- on-disk-effect parity ---------------------------------------------

/// Beyond stdout+exit: run a mutating bash script (`>` redirect + `mv`) on both
/// impls, then read the resulting file back THROUGH each FS seam and assert the
/// persisted bytes match. Proves the mutation path — not just stdout — is a
/// function of the `Filesystem` seam alone.
#[tokio::test]
async fn bash_mutation_ondisk_effect_parity() {
    let script = "echo persisted > a.txt; mv a.txt b.txt";

    // Local impl.
    let fx = WorkspaceFixture::empty();
    let local: Arc<dyn Filesystem> = Arc::new(LocalFilesystem::new(fx.root().to_path_buf()));
    let local_out = bash_on(local.clone(), script).await;

    // Mem impl.
    let mem: Arc<dyn Filesystem> = Arc::new(MemFilesystem::new());
    let mem_out = bash_on(mem.clone(), script).await;

    assert_eq!(local_out.exit_code, 0);
    assert_eq!(mem_out.exit_code, local_out.exit_code, "exit diverged");

    // Source removed by the rename, dest holds the content — on BOTH seams.
    assert!(
        !local.exists(std::path::Path::new("a.txt")).await
            && !mem.exists(std::path::Path::new("a.txt")).await,
        "renamed source must be gone on both impls"
    );
    let local_body = local
        .read(std::path::Path::new("b.txt"), None)
        .await
        .expect("local b.txt readable");
    let mem_body = mem
        .read(std::path::Path::new("b.txt"), None)
        .await
        .expect("mem b.txt readable");
    assert_eq!(
        local_body, mem_body,
        "persisted bytes diverged: local={local_body:?} mem={mem_body:?}"
    );
    assert_eq!(&local_body[..], b"persisted\n");
}
