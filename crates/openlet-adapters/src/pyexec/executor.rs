//! `MontyExecutor` — the `PythonExecutor` impl backed by an in-process
//! [Monty](https://github.com/pydantic/monty) VM instead of a `python3`
//! subprocess.
//!
//! Like `EmulatedShellExecutor`, it holds no workspace root of its own: the
//! workspace lives behind `ctx.fs`, so the same executor runs identically
//! against the local FS or a cloud gRPC backend. There is no process, no host
//! env, no network — Monty is deny-by-default (`os.system`, `socket`,
//! `subprocess` are simply not importable) and every filesystem hop is routed
//! through the [`mount_bridge`](super::mount_bridge) onto `ctx.fs`.
//!
//! The host loop is `async fn`: `MontyRun::start`/`OsCall::resume` are
//! synchronous, and we `.await` the filesystem BETWEEN resumes. No `block_on`,
//! no runtime-in-runtime — the Phase-1 GATE-1b property holds for Python.

use std::time::Duration;

use async_trait::async_trait;
use monty::{
    ExcType, MontyException, MontyObject, MontyRun, NameLookupResult, PrintWriter, ResourceLimits,
    ResourceTracker, RunProgress,
};
use openlet_core::adapters::filesystem::Filesystem;
use openlet_core::adapters::tool_executor::ToolCtx;
use openlet_core::error::ToolError;
use openlet_core::tools::builtins::python::{PythonExecutor, PythonOutput};
use tokio_util::sync::CancellationToken;

use super::mount_bridge::{Dispatched, dispatch_os_call};

/// Default memory budget for one Python run. Generous enough for JSON/string
/// computation over workspace files, tight enough that a `[0]*10**12` bomb
/// trips `max_memory` before the host process feels it.
const DEFAULT_MAX_MEMORY: usize = 256 * 1024 * 1024;

/// Cap on captured stdout / last-expression echo, mirroring the bash executor's
/// `MAX_STDOUT`. Output past this is dropped and `stdout_truncated` is set.
const MAX_STDOUT: usize = 256 * 1024;

/// Stateless executor — everything it needs comes from the per-call `ToolCtx`
/// (the FS seam and the cancel token) plus its configured resource ceiling.
#[derive(Debug, Clone)]
pub struct MontyExecutor {
    max_memory: usize,
}

impl Default for MontyExecutor {
    fn default() -> Self {
        Self {
            max_memory: DEFAULT_MAX_MEMORY,
        }
    }
}

impl MontyExecutor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the per-run memory budget (bytes). Used by tests that want to
    /// prove the memory-bomb guard on a small ceiling.
    #[must_use]
    pub fn with_max_memory(mut self, bytes: usize) -> Self {
        self.max_memory = bytes;
        self
    }
}

#[async_trait]
impl PythonExecutor for MontyExecutor {
    async fn run(
        &self,
        ctx: &ToolCtx,
        code: &str,
        timeout_ms: u64,
    ) -> Result<PythonOutput, ToolError> {
        run_python(
            ctx.fs.as_ref(),
            &ctx.cancel,
            code,
            timeout_ms,
            self.max_memory,
        )
        .await
    }
}

/// Default per-run memory budget for callers that don't build a full
/// `MontyExecutor` (e.g. the emulated shell's `python` builtin). Same ceiling
/// as `MontyExecutor::default`.
#[must_use]
pub fn default_max_memory() -> usize {
    DEFAULT_MAX_MEMORY
}

/// Run Python `code` to completion over the given `Filesystem` seam, returning
/// a `PythonOutput` (stdout / stderr / exit code / timed-out + truncation
/// flags). Shared by [`MontyExecutor`] (the `python` tool) and the emulated
/// shell's `python`/`python3` builtin so both dispatch the SAME interpreter,
/// resource guards, and error shaping — there is exactly one Monty drive loop.
///
/// Takes `fs` + `cancel` directly rather than a `ToolCtx` because the shell
/// builtin only holds those two; the executor touches nothing else.
pub async fn run_python(
    fs: &dyn Filesystem,
    cancel: &CancellationToken,
    code: &str,
    timeout_ms: u64,
    max_memory: usize,
) -> Result<PythonOutput, ToolError> {
    let limits = ResourceLimits::new()
        .max_memory(max_memory)
        .max_duration(Duration::from_millis(timeout_ms));
    let tracker = monty::LimitedTracker::new(limits);

    // stdout is collected into this buffer via `PrintWriter::CollectString`;
    // the module's trailing expression is appended afterwards (REPL-style).
    let mut stdout = String::new();

    let outcome = drive(fs, cancel, code, tracker, &mut stdout).await?;

    let (stdout, stdout_truncated) = cap(stdout);
    match outcome {
        Outcome::Complete(value) => {
            // Echo the module's last expression like a REPL, but skip a
            // bare `None` (a trailing statement / assignment) so we don't
            // print spurious "None" after every write-only script.
            if !matches!(value, MontyObject::None) {
                let rendered = value.to_string();
                let mut merged = stdout;
                if !merged.is_empty() && !merged.ends_with('\n') {
                    merged.push('\n');
                }
                merged.push_str(&rendered);
                if !merged.ends_with('\n') {
                    merged.push('\n');
                }
                let (merged, trunc2) = cap(merged);
                Ok(PythonOutput {
                    stdout: merged,
                    stderr: String::new(),
                    exit_code: 0,
                    timed_out: false,
                    stdout_truncated: stdout_truncated || trunc2,
                    stderr_truncated: false,
                })
            } else {
                Ok(PythonOutput {
                    stdout,
                    stderr: String::new(),
                    exit_code: 0,
                    timed_out: false,
                    stdout_truncated,
                    stderr_truncated: false,
                })
            }
        }
        Outcome::Exception(exc) => {
            // A resource limit (memory / time) surfaces as an uncatchable
            // exception with a distinct `ExcType`; map the time case onto
            // `timed_out` so the runtime treats it like the old subprocess
            // timeout path.
            let timed_out = matches!(exc.exc_type(), ExcType::TimeoutError);
            let (stderr, stderr_truncated) = cap(exc.to_string());
            Ok(PythonOutput {
                stdout,
                stderr,
                exit_code: 1,
                timed_out,
                stdout_truncated,
                stderr_truncated,
            })
        }
    }
}

/// What a finished run produced.
enum Outcome {
    Complete(MontyObject),
    Exception(MontyException),
}

/// Drive one Monty run to completion, resolving every `OsCall` against
/// `fs`. `print` output accumulates into `stdout`.
async fn drive<T: ResourceTracker>(
    fs: &dyn Filesystem,
    cancel: &CancellationToken,
    code: &str,
    tracker: T,
    stdout: &mut String,
) -> Result<Outcome, ToolError> {
    let runner = match MontyRun::new(code.to_owned(), "python", vec![]) {
        Ok(r) => r,
        // A compile error is a user error (bad syntax), not an infrastructure
        // failure — surface it as a Python exception so the caller shapes it
        // into stderr + exit 1 rather than crashing the tool call.
        Err(e) => return Ok(Outcome::Exception(e)),
    };

    let mut progress = match runner.start(vec![], tracker, PrintWriter::CollectString(stdout)) {
        Ok(p) => p,
        Err(e) => return Ok(Outcome::Exception(e)),
    };

    loop {
        // Cancellation is checked at every VM pause (between resumes) — the
        // same contract the bash executor keeps (cancel => Err(Timeout)).
        // NOTE: `MontyRun::start`/`resume` are synchronous, so a purely
        // CPU-bound guest (e.g. `while True: pass`) never yields a pause and
        // cannot be pre-empted by `cancel` mid-compute; the `max_duration`
        // budget below is the backstop that bounds such a guest (and the
        // worker thread it occupies).
        if cancel.is_cancelled() {
            return Err(ToolError::Timeout);
        }

        match progress {
            RunProgress::Complete(value) => return Ok(Outcome::Complete(value)),
            RunProgress::OsCall(mut call) => {
                let fc = call.take_function_call();
                let dispatched = dispatch_os_call(fs, &fc).await;
                let resume: monty::ExtFunctionResult = match dispatched {
                    Dispatched::Ok(obj) => obj.into(),
                    Dispatched::Err(exc) => exc.into(),
                };
                progress = match call.resume(resume, PrintWriter::CollectString(stdout)) {
                    Ok(p) => p,
                    Err(e) => return Ok(Outcome::Exception(e)),
                };
            }
            // An undefined name (e.g. a denied `import socket` surfacing the
            // module name) is a `NameError` by construction — deny it.
            RunProgress::NameLookup(lookup) => {
                progress = match lookup.resume(
                    NameLookupResult::Undefined,
                    PrintWriter::CollectString(stdout),
                ) {
                    Ok(p) => p,
                    Err(e) => return Ok(Outcome::Exception(e)),
                };
            }
            // We register no external functions and no async futures, so these
            // pauses cannot occur; treat them as an interpreter contract
            // violation rather than silently looping.
            RunProgress::FunctionCall(_) | RunProgress::ResolveFutures(_) => {
                return Ok(Outcome::Exception(MontyException::new(
                    ExcType::RuntimeError,
                    Some("python: unexpected external-call pause".to_string()),
                )));
            }
        }
    }
}

/// Truncate `s` to `MAX_STDOUT` bytes on a char boundary, reporting whether it
/// was cut.
fn cap(mut s: String) -> (String, bool) {
    if s.len() <= MAX_STDOUT {
        return (s, false);
    }
    let mut end = MAX_STDOUT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s.truncate(end);
    (s, true)
}
