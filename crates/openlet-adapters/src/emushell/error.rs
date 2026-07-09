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
