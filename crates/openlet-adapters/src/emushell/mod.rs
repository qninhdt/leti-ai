//! Emulated bash — an in-process async shell interpreter.
//!
//! Replaces the subprocess-based `LocalShellExecutor` with a shell built
//! on `tree-sitter-bash` (parse → CST) plus a hand-written async evaluator
//! that walks a typed AST. Every command is a builtin; every IO hop goes
//! through the injected [`Filesystem`](openlet_core::adapters::filesystem::Filesystem).
//!
//! This is *security by construction*, not isolation: the interpreter has
//! no code path that spawns a process, opens a socket, or touches the host
//! filesystem directly, so there is nothing to sandbox. An unknown command
//! is `command not found` (exit 127) because the dispatch table has no
//! host-exec fallback.
//!
//! Module layout:
//! - `ast`      — the typed shell AST the evaluator walks
//! - `parse`    — lowers the tree-sitter CST into that AST
//! - `env`      — shell variables + `$var` / glob expansion
//! - `eval`     — the async evaluator (the CST walk)
//! - `builtins` — the emulated coreutils command set
//! - `executor` — the `ShellExecutor` trait impl the runtime injects

mod ast;
mod builtins;
mod env;
mod error;
mod eval;
mod executor;
mod parse;

pub use error::ShellParseError;
pub use executor::EmulatedShellExecutor;
