//! Interpreter-infrastructure errors.
//!
//! These are the ONLY things that surface as `Err(ToolError)` from the
//! executor. Everything a running script can do wrong — a missing file,
//! a bad glob, an unknown command — becomes stderr text + a non-zero
//! exit code INSIDE the `BashOutput`, never an `Err`. Reserving `Err`
//! for genuine infrastructure faults keeps the tool contract intact: the
//! TUI renders a shell result, not a tool crash, whenever the shell ran
//! at all.

use std::fmt;

use leti_core::error::FsError;

/// Human-readable one-liner for an `FsError`, matching the shape a real
/// coreutil prints (`No such file or directory`, etc.) so the LLM sees a
/// familiar message. Shared by the builtins dispatch table and the
/// evaluator's redirection paths (previously duplicated in both).
#[must_use]
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

/// Failure to parse a script into the AST (grammar load failure, or the
/// parser returning no tree). A syntax error in the script itself is NOT
/// this — tree-sitter is error-tolerant and yields a partial tree we run.
#[derive(Debug, Clone)]
pub struct ShellParseError(pub String);

impl fmt::Display for ShellParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "shell parse error: {}", self.0)
    }
}

impl std::error::Error for ShellParseError {}
