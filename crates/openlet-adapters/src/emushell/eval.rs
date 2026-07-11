//! Async evaluator: walk the typed [`Script`] AST, running every command
//! as a builtin and routing every IO hop through `ctx.fs`.
//!
//! Security by construction: there is no branch that spawns a process or
//! opens a socket. An unknown command name resolves to exit code 127
//! (`command not found`) because the dispatch table simply has no entry
//! and no fallback to the host. File reads/writes go through the injected
//! [`Filesystem`]; a path outside the workspace is rejected by the trait,
//! surfacing here as a non-zero exit + stderr line — never a host escape.
//!
//! The eval loop is `async fn` throughout and awaits `ctx.fs.*` directly,
//! so there is no sync-to-async bridge and no `block_on`. Recursive nodes
//! (`for`/`while`/`if` bodies, `$(...)` re-entry, pipelines) box their
//! futures to keep each `async fn` sized.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::time::{Duration, Instant};

use openlet_core::adapters::filesystem::{Filesystem, WriteOpts};
use tokio_util::sync::CancellationToken;

use super::ast::{AndOr, AndOrOp, Command, Frag, Node, Script, Word};
use super::builtins::{self, BuiltinCtx};
use super::env::{ShellEnv, expand_glob};

/// Output byte caps — reused from the previous subprocess executor so the
/// emulated shell truncates at the same thresholds.
const MAX_STDOUT: usize = 256 * 1024;
const MAX_STDERR: usize = 64 * 1024;

/// Ceiling on evaluated nodes per `run()`. A `while true` loop or a fork
/// bomb of `$(...)` re-entries would otherwise spin forever inside the
/// process. This is the count-based backstop; a wall-clock deadline
/// (`timeout_ms`) runs alongside it so a loop whose bodies are cheap but
/// numerous still stops in bounded real time.
const MAX_STEPS: u64 = 5_000_000;

/// How often a `while` loop yields back to the tokio runtime. A tight loop
/// whose body never `.await`s (e.g. `while true; do :; done`) would otherwise
/// hold the worker thread until the deadline; yielding periodically lets the
/// runtime observe cancellation and service co-scheduled tasks. The in-band
/// deadline in `tick` still bounds total wall-clock regardless.
const YIELD_EVERY: u64 = 1024;

/// Result of running one command or node: what it wrote to stdout plus its
/// exit status. Stderr is accumulated on the interpreter, not returned,
/// because it is never piped.
struct CmdOut {
    stdout: String,
    status: i32,
}

/// One interpreter run. Holds the FS seam, the cancel token, mutable shell
/// state, and the accumulating output buffers.
pub struct Interp<'a> {
    fs: &'a dyn Filesystem,
    cancel: &'a CancellationToken,
    env: ShellEnv,
    stdout: String,
    stderr: String,
    stdout_truncated: bool,
    stderr_truncated: bool,
    steps: u64,
    /// Wall-clock cutoff for the whole run. `None` disables the time guard
    /// (the step budget still applies). Checked in-band from `tick` so a
    /// pure-CPU loop that never `.await`s the filesystem is still bounded in
    /// real time — a wrapping `tokio::time::timeout` alone cannot pre-empt
    /// such a loop because it never yields.
    deadline: Option<Instant>,
    /// Set when the step budget is exhausted, the deadline passed, or the run
    /// is cancelled — the loop unwinds without treating it as a normal
    /// command failure.
    aborted: Option<AbortReason>,
}

#[derive(Debug, Clone, Copy)]
pub enum AbortReason {
    /// Evaluated-node count exceeded `MAX_STEPS`.
    StepBudget,
    /// Wall-clock `timeout_ms` elapsed.
    Timeout,
    /// The runtime cancelled the run via `ctx.cancel`.
    Cancelled,
}

/// Final result of a run — mirrors the fields `BashOutput` needs, plus the
/// step count so a parent interpreter can fold a child `$(...)` run's steps
/// back into its own budget (otherwise repeated cmdsubs each get a fresh
/// budget and the DoS guard does not compose).
pub struct RunResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub aborted: Option<AbortReason>,
    pub steps: u64,
}

impl<'a> Interp<'a> {
    pub fn new(fs: &'a dyn Filesystem, cancel: &'a CancellationToken) -> Self {
        Self {
            fs,
            cancel,
            env: ShellEnv::new(),
            stdout: String::new(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            steps: 0,
            deadline: None,
            aborted: None,
        }
    }

    /// Arm a wall-clock deadline `timeout` from now. The deadline is checked
    /// in-band on every `tick`, so it bounds even a pure-CPU loop that never
    /// awaits the filesystem (which a wrapping `tokio::time::timeout` cannot).
    ///
    /// A zero timeout disarms the deadline rather than aborting instantly —
    /// callers that want "no time limit" pass `Duration::ZERO`. An absurdly
    /// large `timeout` that would overflow `Instant` also disarms it (the step
    /// budget remains the backstop) instead of panicking.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.deadline = if timeout.is_zero() {
            None
        } else {
            Instant::now().checked_add(timeout)
        };
        self
    }

    /// Run a whole script, returning the accumulated output + last status.
    pub async fn run(mut self, script: &Script) -> RunResult {
        let status = self.run_script(script).await;
        RunResult {
            stdout: self.stdout,
            stderr: self.stderr,
            exit_code: status,
            stdout_truncated: self.stdout_truncated,
            stderr_truncated: self.stderr_truncated,
            aborted: self.aborted,
            steps: self.steps,
        }
    }

    fn run_script<'s>(
        &'s mut self,
        script: &'s Script,
    ) -> Pin<Box<dyn Future<Output = i32> + Send + 's>> {
        Box::pin(async move {
            let mut status = 0;
            for item in &script.items {
                if self.check_abort() {
                    break;
                }
                status = self.run_andor(item).await;
            }
            status
        })
    }

    fn run_andor<'s>(
        &'s mut self,
        andor: &'s AndOr,
    ) -> Pin<Box<dyn Future<Output = i32> + Send + 's>> {
        Box::pin(async move {
            let mut status = self.run_node(&andor.head).await;
            for (op, node) in &andor.tail {
                if self.check_abort() {
                    break;
                }
                let should_run = match op {
                    AndOrOp::And => status == 0,
                    AndOrOp::Or => status != 0,
                };
                if should_run {
                    status = self.run_node(node).await;
                }
            }
            self.env.set_last_status(status);
            status
        })
    }

    fn run_node<'s>(
        &'s mut self,
        node: &'s Node,
    ) -> Pin<Box<dyn Future<Output = i32> + Send + 's>> {
        Box::pin(async move {
            match node {
                Node::Pipeline(cmds) => self.run_pipeline(cmds).await,
                Node::For { var, items, body } => self.run_for(var, items, body).await,
                Node::While { cond, body } => self.run_while(cond, body).await,
                Node::If {
                    cond,
                    then_body,
                    else_body,
                } => self.run_if(cond, then_body, else_body.as_deref()).await,
            }
        })
    }

    async fn run_pipeline(&mut self, cmds: &[Command]) -> i32 {
        if cmds.is_empty() {
            return 0;
        }
        let mut stdin = String::new();
        let mut status = 0;
        for (idx, cmd) in cmds.iter().enumerate() {
            if self.check_abort() {
                return 1;
            }
            let is_last = idx == cmds.len() - 1;
            let out = self.run_command(cmd, &stdin).await;
            status = out.status;
            if is_last {
                // Last command's stdout is the pipeline's stdout — unless the
                // command redirected it to a file (handled in run_command,
                // which returns empty stdout in that case).
                self.push_stdout(&out.stdout);
            } else {
                // Feed this command's stdout into the next as stdin.
                stdin = out.stdout;
            }
        }
        status
    }

    async fn run_command(&mut self, cmd: &Command, stdin: &str) -> CmdOut {
        self.tick();
        // Assignments with no argv: set vars, exit 0.
        let argv = self.expand_command_words(cmd).await;
        if argv.is_empty() {
            for (name, word) in &cmd.assignments {
                let val = self.expand_word_to_string(word).await;
                self.env.set(name, val);
            }
            return CmdOut {
                stdout: String::new(),
                status: 0,
            };
        }

        // Resolve stdin: an explicit `< file` overrides piped stdin.
        let effective_stdin = if let Some(w) = &cmd.redirect_in {
            let path = self.expand_word_to_string(w).await;
            match self.fs.read(Path::new(&path), None).await {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(e) => {
                    self.push_stderr(&format!("{}: {}\n", argv[0], fs_err_msg(&e)));
                    return CmdOut {
                        stdout: String::new(),
                        status: 1,
                    };
                }
            }
        } else {
            stdin.to_string()
        };

        let (out, status) = self.run_builtin(&argv, &effective_stdin).await;

        // Output redirect: write stdout to file instead of surfacing it.
        if let Some((word, append)) = &cmd.redirect_out {
            let path = self.expand_word_to_string(word).await;
            let opts = WriteOpts {
                append: *append,
                ..WriteOpts::default()
            };
            match self
                .fs
                .write(Path::new(&path), bytes::Bytes::from(out.into_bytes()), opts)
                .await
            {
                Ok(_) => CmdOut {
                    stdout: String::new(),
                    status,
                },
                Err(e) => {
                    self.push_stderr(&format!("{}: {}\n", argv[0], fs_err_msg(&e)));
                    CmdOut {
                        stdout: String::new(),
                        status: 1,
                    }
                }
            }
        } else {
            CmdOut {
                stdout: out,
                status,
            }
        }
    }

    /// Run one simple command through the builtin registry, folding its
    /// stderr onto the interpreter's buffer and returning `(stdout, status)`.
    /// The registry itself has no exec branch — an unknown name comes back as
    /// exit 127 (`command not found`), the deny-by-construction fallback.
    async fn run_builtin(&mut self, argv: &[String], stdin: &str) -> (String, i32) {
        let bctx = BuiltinCtx {
            fs: self.fs,
            cancel: self.cancel,
        };
        let result = builtins::dispatch(&bctx, argv, stdin).await;
        if !result.stderr.is_empty() {
            self.push_stderr(&result.stderr);
        }
        (result.stdout, result.status)
    }

    async fn run_for(&mut self, var: &str, items: &[Word], body: &Script) -> i32 {
        self.tick();
        let mut expanded = Vec::new();
        for w in items {
            expanded.extend(self.expand_word(w).await);
        }
        let mut status = 0;
        for item in expanded {
            if self.check_abort() {
                break;
            }
            self.env.set(var, item);
            status = self.run_script(body).await;
        }
        status
    }

    async fn run_while(&mut self, cond: &Script, body: &Script) -> i32 {
        let mut status = 0;
        let mut iters: u64 = 0;
        loop {
            if self.check_abort() {
                break;
            }
            self.tick();
            // A `while true` body may never touch the filesystem, so nothing
            // in it `.await`s and the tokio worker would be monopolised until
            // the deadline. Yield cooperatively every so often so the runtime
            // can service other tasks (and observe cancellation) while the
            // in-band deadline still bounds total wall-clock.
            iters += 1;
            if iters.is_multiple_of(YIELD_EVERY) {
                tokio::task::yield_now().await;
            }
            let cond_status = self.run_script(cond).await;
            if cond_status != 0 {
                break;
            }
            status = self.run_script(body).await;
        }
        status
    }

    async fn run_if(
        &mut self,
        cond: &Script,
        then_body: &Script,
        else_body: Option<&Script>,
    ) -> i32 {
        self.tick();
        let cond_status = self.run_script(cond).await;
        if cond_status == 0 {
            self.run_script(then_body).await
        } else if let Some(eb) = else_body {
            self.run_script(eb).await
        } else {
            0
        }
    }

    // --- word expansion --------------------------------------------------

    /// Expand every word of a command's argv (var-sub, `$(...)`, glob).
    async fn expand_command_words(&mut self, cmd: &Command) -> Vec<String> {
        let mut argv = Vec::new();
        for word in &cmd.words {
            argv.extend(self.expand_word(word).await);
        }
        argv
    }

    /// Expand a single word into zero or more argv tokens. Command
    /// substitution re-enters the evaluator; glob hits `ctx.fs`.
    fn expand_word<'s>(
        &'s mut self,
        word: &'s Word,
    ) -> Pin<Box<dyn Future<Output = Vec<String>> + Send + 's>> {
        Box::pin(async move {
            let (text, globbable) = self.assemble_word(word).await;
            if globbable && has_glob_meta(&text) {
                expand_glob(self.fs, &text).await
            } else {
                vec![text]
            }
        })
    }

    /// Same as `expand_word` but collapses to a single string (for redirect
    /// targets and assignment values — no word-splitting, no globbing).
    async fn expand_word_to_string(&mut self, word: &Word) -> String {
        self.assemble_word(word).await.0
    }

    /// Concatenate a word's fragments into a final string. Returns whether
    /// the result is glob-eligible (true only if some unquoted literal
    /// fragment carried a metacharacter).
    fn assemble_word<'s>(
        &'s mut self,
        word: &'s Word,
    ) -> Pin<Box<dyn Future<Output = (String, bool)> + Send + 's>> {
        Box::pin(async move {
            let mut out = String::new();
            let mut globbable = false;
            for frag in &word.frags {
                match frag {
                    Frag::Literal(s) => {
                        if s.contains('*') || s.contains('?') || s.contains('[') {
                            globbable = true;
                        }
                        out.push_str(&self.env.expand_vars(s));
                    }
                    Frag::Quoted(s) => out.push_str(s),
                    Frag::Var(name) => out.push_str(&self.env.expand_vars(&format!("${{{name}}}"))),
                    Frag::CmdSub(script) => {
                        // Re-enter eval on a child interpreter sharing the FS +
                        // cancel + a snapshot of vars; capture its stdout,
                        // strip the trailing newline like bash does.
                        let captured = self.run_cmdsub(script).await;
                        out.push_str(captured.trim_end_matches('\n'));
                    }
                }
            }
            (out, globbable)
        })
    }

    /// Run a `$(...)` body and return its stdout. Shares var state by value
    /// (child sees parent vars; assignments inside do not leak back, which
    /// matches bash subshell semantics).
    fn run_cmdsub<'s>(
        &'s mut self,
        script: &'s Script,
    ) -> Pin<Box<dyn Future<Output = String> + Send + 's>> {
        Box::pin(async move {
            self.tick();
            if self.aborted.is_some() {
                return String::new();
            }
            let mut child = Interp::new(self.fs, self.cancel);
            child.env = self.env.clone();
            child.steps = self.steps;
            // The child shares the parent's wall-clock deadline so a `$(...)`
            // that spins does not get a fresh, unbounded time budget.
            child.deadline = self.deadline;
            let result = child.run(script).await;
            // Propagate step accounting + abort back to the parent so a
            // `$(...)` bomb still trips the shared budget: the child starts
            // at the parent's count and returns the total it reached, so
            // repeated/sibling cmdsubs cannot each get a fresh budget.
            self.steps = result.steps;
            if let Some(reason) = result.aborted {
                self.aborted = Some(reason);
            }
            // Subshell stderr surfaces on the parent.
            self.push_stderr(&result.stderr);
            result.stdout
        })
    }

    // --- budget / cancel / output caps -----------------------------------

    fn tick(&mut self) {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            self.aborted = Some(AbortReason::StepBudget);
        }
        // In-band wall-clock check: a tight CPU loop (`while true; do :; done`)
        // never `.await`s the filesystem, so a wrapping `tokio::time::timeout`
        // can never pre-empt it. Checking the deadline here — at every
        // evaluated node — bounds such a loop in real time regardless.
        if let Some(deadline) = self.deadline
            && Instant::now() >= deadline
        {
            self.aborted = Some(AbortReason::Timeout);
        }
        if self.cancel.is_cancelled() {
            self.aborted = Some(AbortReason::Cancelled);
        }
    }

    fn check_abort(&mut self) -> bool {
        if self.cancel.is_cancelled() {
            self.aborted = Some(AbortReason::Cancelled);
        }
        self.aborted.is_some()
    }

    fn push_stdout(&mut self, s: &str) {
        push_capped(&mut self.stdout, &mut self.stdout_truncated, s, MAX_STDOUT);
    }

    fn push_stderr(&mut self, s: &str) {
        push_capped(&mut self.stderr, &mut self.stderr_truncated, s, MAX_STDERR);
    }
}

fn push_capped(buf: &mut String, truncated: &mut bool, s: &str, cap: usize) {
    if *truncated {
        return;
    }
    let room = cap.saturating_sub(buf.len());
    if s.len() <= room {
        buf.push_str(s);
    } else {
        // Floor the cut to a UTF-8 char boundary so slicing never panics
        // mid-codepoint on multibyte output (e.g. `cat` on a Unicode file).
        let mut cut = room.min(s.len());
        while cut > 0 && !s.is_char_boundary(cut) {
            cut -= 1;
        }
        buf.push_str(&s[..cut]);
        *truncated = true;
    }
}

fn has_glob_meta(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '*' | '?' | '['))
}

/// Human-readable one-liner for an `FsError`, matching the shape a real
/// coreutil prints (`No such file or directory`, etc.) so the LLM sees a
/// familiar message.
fn fs_err_msg(e: &openlet_core::error::FsError) -> String {
    use openlet_core::error::FsError;
    match e {
        FsError::NotFound(p) => format!("{p}: No such file or directory"),
        FsError::OutsideWorkspace(p) => format!("{p}: Permission denied"),
        FsError::Binary(p) => format!("{p}: binary file"),
        FsError::TooLarge { path, .. } => format!("{path}: file too large"),
        FsError::InvalidInput(m) | FsError::Io(m) => m.clone(),
        FsError::Unsupported(m) => format!("operation not supported: {m}"),
    }
}
