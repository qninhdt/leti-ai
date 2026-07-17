//! Word expansion for the emulated-shell evaluator: variable substitution,
//! `$(...)` command substitution (re-enters the evaluator), and globbing.
//!
//! Extracted from `eval/mod.rs` as a cohesive unit. These are `Interp`
//! methods; a submodule can still reach the parent struct's private fields,
//! so the split is purely organizational — no behavior change.

use std::future::Future;
use std::pin::Pin;

use super::super::ast::{Command, Frag, Script, Word};
use super::super::env::{expand_glob, has_glob_meta};
use super::Interp;

impl Interp<'_> {
    /// Expand every word of a command's argv (var-sub, `$(...)`, glob).
    pub(super) async fn expand_command_words(&mut self, cmd: &Command) -> Vec<String> {
        let mut argv = Vec::new();
        for word in &cmd.words {
            argv.extend(self.expand_word(word).await);
        }
        argv
    }

    /// Expand a single word into zero or more argv tokens. Command
    /// substitution re-enters the evaluator; glob hits `ctx.fs`.
    pub(super) fn expand_word<'s>(
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
    pub(super) async fn expand_word_to_string(&mut self, word: &Word) -> String {
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
}
