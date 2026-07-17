//! Emulated coreutils builtins.
//!
//! Each builtin is a free `async fn` taking a [`BuiltinCtx`] (the FS seam +
//! cancel token), the already-expanded `argv`, and the piped `stdin`. It
//! returns a [`BuiltinResult`] — stdout, stderr, and an exit code. There is
//! no branch that spawns a process or opens a socket: every IO hop goes
//! through `ctx.fs`, and an unknown command name resolves to exit 127
//! (`command not found`) because [`dispatch`] simply has no entry for it.
//!
//! Grouped by data-flow so each file stays small:
//! - `transform_ops` — pure stream/arg transforms (echo, sort, uniq, …).
//! - `text_ops`       — single-file content reads (cat, head, tail, …).
//! - `sed` / `awk`   — the sed / awk 80/20 subsets.
//! - `tree_ops`       — directory + content search (ls, find, grep).
//! - `mutation_ops`   — writes / deletes / moves (mkdir, rm, mv, cp, …).
//! - `python_op`      — `python`/`python3` routed to the shared Monty
//!   interpreter (the ONE exception to "coreutils only"; still sandboxed,
//!   still `ctx.fs`-only, no host process).

mod awk;
mod mutation_ops;
mod python_op;
mod sed;
mod text_ops;
mod transform_ops;
mod tree_ops;

use leti_core::adapters::filesystem::Filesystem;
use tokio_util::sync::CancellationToken;

/// Seam handed to every builtin: the injected filesystem and the run's
/// cancel token. Deliberately tiny — a builtin can only touch the
/// workspace through `fs`, never the host.
pub struct BuiltinCtx<'a> {
    pub fs: &'a dyn Filesystem,
    pub cancel: &'a CancellationToken,
}

/// What one builtin produced. `stdout` feeds the next stage of a pipeline;
/// `stderr` accumulates on the interpreter (never piped); `status` is the
/// exit code.
pub struct BuiltinResult {
    pub stdout: String,
    pub stderr: String,
    pub status: i32,
}

impl BuiltinResult {
    /// Success with stdout and no stderr.
    fn out(stdout: String) -> Self {
        Self {
            stdout,
            stderr: String::new(),
            status: 0,
        }
    }

    /// Failure: a stderr line (newline appended) and a non-zero code.
    fn err(msg: impl Into<String>, status: i32) -> Self {
        let mut stderr = msg.into();
        if !stderr.ends_with('\n') {
            stderr.push('\n');
        }
        Self {
            stdout: String::new(),
            stderr,
            status,
        }
    }
}

/// Resolve a command name to its builtin and run it. Unknown names are
/// `command not found` (exit 127) — the deny-by-construction fallback.
pub async fn dispatch(ctx: &BuiltinCtx<'_>, argv: &[String], stdin: &str) -> BuiltinResult {
    let name = argv[0].as_str();
    match name {
        // transform_ops
        "echo" => transform_ops::echo(argv),
        "true" => BuiltinResult::out(String::new()),
        "false" => BuiltinResult {
            stdout: String::new(),
            stderr: String::new(),
            status: 1,
        },
        "sort" => transform_ops::sort(ctx, argv, stdin).await,
        "uniq" => transform_ops::uniq(ctx, argv, stdin).await,
        "basename" => transform_ops::basename(argv),
        "dirname" => transform_ops::dirname(argv),
        "diff" => transform_ops::diff(ctx, argv, stdin).await,
        "xargs" => transform_ops::xargs(ctx, argv, stdin).await,
        // text_ops
        "cat" => text_ops::cat(ctx, argv, stdin).await,
        "head" => text_ops::head(ctx, argv, stdin).await,
        "tail" => text_ops::tail(ctx, argv, stdin).await,
        "wc" => text_ops::wc(ctx, argv, stdin).await,
        "cut" => text_ops::cut(ctx, argv, stdin).await,
        "tr" => text_ops::tr(argv, stdin),
        // sed / awk
        "sed" => sed::sed(ctx, argv, stdin).await,
        "awk" => awk::awk(ctx, argv, stdin).await,
        // tree_ops
        "ls" => tree_ops::ls(ctx, argv).await,
        "find" => tree_ops::find(ctx, argv).await,
        "grep" => tree_ops::grep(ctx, argv, stdin).await,
        // mutation_ops
        "mkdir" => mutation_ops::mkdir(ctx, argv).await,
        "rm" => mutation_ops::rm(ctx, argv).await,
        "mv" => mutation_ops::mv(ctx, argv).await,
        "cp" => mutation_ops::cp(ctx, argv).await,
        "touch" => mutation_ops::touch(ctx, argv).await,
        "tee" => mutation_ops::tee(ctx, argv, stdin).await,
        // python interpreter (Monty) — same in-process executor the `python`
        // tool uses, routed through `ctx.fs`. NOT a subprocess.
        "python" | "python3" => python_op::python(ctx, argv, stdin).await,
        other => {
            // Security by construction: no exec branch. Unknown command is
            // exactly bash's own message + exit 127.
            BuiltinResult::err(format!("{other}: command not found"), 127)
        }
    }
}

/// Re-exported from [`super::error`] — the single canonical `fs_err_msg`.
/// Builtins reach it via `super::fs_err_msg` (unchanged call sites).
pub(crate) use super::error::fs_err_msg;

/// Read file operands into one buffer, else return `stdin`. Shared by
/// every content builtin (cat/head/tail/wc/cut/sort/uniq/sed/awk) — the
/// single canonical copy of what used to be triplicated as `gather`
/// (text_ops, sed_awk) and `gather_input` (transform_ops).
pub(super) async fn gather(
    ctx: &BuiltinCtx<'_>,
    name: &str,
    files: &[String],
    stdin: &str,
) -> Result<String, BuiltinResult> {
    if files.is_empty() {
        return Ok(stdin.to_string());
    }
    let mut out = String::new();
    for f in files {
        match ctx.fs.read(std::path::Path::new(f), None).await {
            Ok(bytes) => out.push_str(&String::from_utf8_lossy(&bytes)),
            Err(e) => return Err(BuiltinResult::err(format!("{name}: {}", fs_err_msg(&e)), 1)),
        }
    }
    Ok(out)
}

/// Split a flag-cluster arg like `-la` into individual short flags. Returns
/// `None` if `arg` is not a short-flag cluster (does not start with a single
/// `-`, or is `-` / `--`).
pub(crate) fn short_flags(arg: &str) -> Option<Vec<char>> {
    if arg.len() < 2 || !arg.starts_with('-') || arg.starts_with("--") {
        return None;
    }
    Some(arg[1..].chars().collect())
}
