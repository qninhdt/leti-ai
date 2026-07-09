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
//! - `sed_awk`        — the sed / awk 80/20 subsets.
//! - `tree_ops`       — directory + content search (ls, find, grep).
//! - `mutation_ops`   — writes / deletes / moves (mkdir, rm, mv, cp, …).

mod mutation_ops;
mod sed_awk;
mod text_ops;
mod transform_ops;
mod tree_ops;

use openlet_core::adapters::filesystem::Filesystem;
use openlet_core::error::FsError;
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
        "sed" => sed_awk::sed(ctx, argv, stdin).await,
        "awk" => sed_awk::awk(ctx, argv, stdin).await,
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
        other => {
            // Security by construction: no exec branch. Unknown command is
            // exactly bash's own message + exit 127.
            BuiltinResult::err(format!("{other}: command not found"), 127)
        }
    }
}

/// Human-readable one-liner for an `FsError`, matching the shape a real
/// coreutil prints (`No such file or directory`, etc.) so the LLM sees a
/// familiar message.
pub(crate) fn fs_err_msg(e: &FsError) -> String {
    match e {
        FsError::NotFound(p) => format!("{p}: No such file or directory"),
        FsError::OutsideWorkspace(p) => format!("{p}: Permission denied"),
        FsError::Binary(p) => format!("{p}: binary file"),
        FsError::TooLarge { path, .. } => format!("{path}: file too large"),
        FsError::InvalidInput(m) | FsError::Io(m) => m.clone(),
        FsError::Unsupported(m) => format!("operation not supported: {m}"),
    }
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
