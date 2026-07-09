//! `python` / `python3` builtin — runs Python through the SAME in-process
//! Monty interpreter the `python` tool uses, over `ctx.fs`.
//!
//! This is the one builtin that is not a coreutil: it exists so an LLM that
//! reflexively types `python3 script.py` in the `bash` tool gets a real run
//! instead of `command not found`. It routes to [`run_python`] — the exact
//! same drive loop, resource guards, and deny-by-construction sandbox as the
//! standalone `python` tool — so there is no second Python code path to keep
//! secure.
//!
//! Supported invocation forms (what real `python3` does AND Monty can honor):
//! - `python -c "CODE"`   — run the inline program.
//! - `python script.py`   — read the file through `ctx.fs`, run its contents.
//! - `... | python3`      — no file / no `-c`: run piped stdin as the program.
//!
//! Deliberately NOT emulated (Monty limitation, surfaced loudly, never
//! silently wrong):
//! - `sys.argv`: Monty's `sys` module exposes no `argv`, so extra operands
//!   after the script are accepted but a program that reads `sys.argv` raises
//!   `AttributeError` at runtime (a real, visible error — not a wrong answer).
//! - REPL / `-i`, `-m module`, `--version`: rejected with a clear message.

use std::path::Path;

use crate::pyexec::{default_max_memory, run_python};

use super::{fs_err_msg, BuiltinCtx, BuiltinResult};

/// Default wall-clock budget for a `python` builtin run. The bash interpreter
/// enforces its own overall deadline in-band; this bounds the Monty guest's
/// `max_duration` so a `while True: pass` inside the guest can't monopolize the
/// worker thread indefinitely. Mirrors the `python` tool's 30s default.
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// `python [-c CODE | script.py] [args...]` — see module docs.
pub(super) async fn python(ctx: &BuiltinCtx<'_>, argv: &[String], stdin: &str) -> BuiltinResult {
    let name = argv[0].as_str();

    let code = match resolve_source(ctx, name, &argv[1..], stdin).await {
        Ok(c) => c,
        Err(e) => return e,
    };

    let out = run_python(ctx.fs, ctx.cancel, &code, DEFAULT_TIMEOUT_MS, default_max_memory()).await;
    match out {
        Ok(py) => {
            // `run_python` already shapes guest exceptions into stderr + exit 1
            // and resource trips into `timed_out`. Fold those into the builtin
            // result shape (stdout / stderr / status); a timed-out run gets a
            // naming stderr line like the bash DoS guard.
            let mut stderr = py.stderr;
            if py.timed_out {
                stderr.push_str(&format!(
                    "{name}: terminated: resource limit ({DEFAULT_TIMEOUT_MS}ms / memory)\n"
                ));
            }
            BuiltinResult {
                stdout: py.stdout,
                stderr,
                status: py.exit_code,
            }
        }
        // The only `Err` path is cancellation (Err(Timeout)); surface it as the
        // shell interrupt code so the interpreter unwinds like any cancel.
        Err(_) => BuiltinResult::err(format!("{name}: interrupted"), 130),
    }
}

/// Determine the Python program text from the argument form. Returns the
/// program source on success, or a ready-to-return error `BuiltinResult`.
///
/// The first operand selects the form: `-c` takes the next operand as the
/// program; a script path is read through `ctx.fs`; a bare `-` (or no operand
/// at all) falls back to stdin. Unsupported flags fail loud rather than being
/// mistaken for a script path.
async fn resolve_source(
    ctx: &BuiltinCtx<'_>,
    name: &str,
    args: &[String],
    stdin: &str,
) -> Result<String, BuiltinResult> {
    let Some(first) = args.first() else {
        // No `-c`, no script operand → run stdin as the program (real python3
        // reads a program from stdin when given nothing else).
        return Ok(stdin.to_string());
    };
    match first.as_str() {
        "-c" => args.get(1).cloned().ok_or_else(|| {
            BuiltinResult::err(format!("{name}: option -c requires an argument"), 2)
        }),
        "-m" => Err(BuiltinResult::err(
            format!("{name}: -m (run module) is not supported by the embedded interpreter"),
            2,
        )),
        "-i" => Err(BuiltinResult::err(
            format!("{name}: -i (interactive REPL) is not supported"),
            2,
        )),
        "--version" | "-V" => Err(BuiltinResult::err(
            format!("{name}: --version is not supported by the embedded interpreter"),
            2,
        )),
        // Bare `-` = read the program from stdin (like real python3).
        "-" => Ok(stdin.to_string()),
        // A leading `-x` we don't recognize: reject loudly rather than
        // treating it as a script path.
        s if s.starts_with('-') => {
            Err(BuiltinResult::err(format!("{name}: unsupported option: {s}"), 2))
        }
        // First non-flag operand is the script path; any trailing operands are
        // its argv (unused — see module docs on the `sys.argv` limitation).
        s => read_script(ctx, name, s).await,
    }
}

/// Read a script file through `ctx.fs` (never the host disk).
async fn read_script(
    ctx: &BuiltinCtx<'_>,
    name: &str,
    path: &str,
) -> Result<String, BuiltinResult> {
    match ctx.fs.read(Path::new(path), None).await {
        Ok(bytes) => String::from_utf8(bytes.to_vec())
            .map_err(|_| BuiltinResult::err(format!("{name}: {path}: not a text file"), 1)),
        Err(e) => Err(BuiltinResult::err(format!("{name}: {}", fs_err_msg(&e)), 2)),
    }
}
