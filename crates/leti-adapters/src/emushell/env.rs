//! Shell environment + word expansion.
//!
//! Holds shell variables and performs word expansion: `$var` / `${var}`
//! substitution, tilde-free (no `$HOME` shell here), and glob expansion
//! routed through `ctx.fs.glob` — NEVER the host filesystem. Command
//! substitution `$(...)` is handled by the evaluator (it re-enters eval),
//! so this module exposes a hook the evaluator fills.

use std::collections::BTreeMap;

use leti_core::adapters::filesystem::{Filesystem, GlobOpts};

/// Shell variable environment. Distinct from process env — the emulated
/// shell has no access to real environment variables (security by
/// construction: they are never wired in). Variables set here live only
/// for the duration of one `run()`.
#[derive(Debug, Default, Clone)]
pub struct ShellEnv {
    vars: BTreeMap<String, String>,
    /// Exit code of the last command — surfaces as `$?`.
    last_status: i32,
}

impl ShellEnv {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, name: &str, value: String) {
        self.vars.insert(name.to_string(), value);
    }

    pub fn set_last_status(&mut self, code: i32) {
        self.last_status = code;
    }

    /// Substitute `$var` / `${var}` / `$?` inside a raw string. Unknown
    /// vars expand to empty (bash default without `set -u`). Command
    /// substitution is NOT handled here — the evaluator resolves those
    /// before calling this (the `subst` map carries pre-computed results).
    #[must_use]
    pub fn expand_vars(&self, raw: &str) -> String {
        let mut out = String::new();
        let mut chars = raw.chars().peekable();
        while let Some(c) = chars.next() {
            if c != '$' {
                out.push(c);
                continue;
            }
            match chars.peek() {
                Some('{') => {
                    chars.next(); // consume '{'
                    let mut name = String::new();
                    for nc in chars.by_ref() {
                        if nc == '}' {
                            break;
                        }
                        name.push(nc);
                    }
                    out.push_str(self.lookup(&name));
                }
                Some('?') => {
                    chars.next();
                    out.push_str(&self.last_status.to_string());
                }
                Some(c2) if c2.is_ascii_alphabetic() || *c2 == '_' => {
                    let mut name = String::new();
                    while let Some(&nc) = chars.peek() {
                        if nc.is_ascii_alphanumeric() || nc == '_' {
                            name.push(nc);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    out.push_str(self.lookup(&name));
                }
                // `$` not followed by a name — literal dollar.
                _ => out.push('$'),
            }
        }
        out
    }

    fn lookup(&self, name: &str) -> &str {
        if name == "?" {
            // handled inline, but keep the branch total
            return "";
        }
        self.vars.get(name).map(String::as_str).unwrap_or("")
    }
}

/// Expand one already-var-substituted word into zero or more argv tokens.
///
/// - A word with no glob metacharacters expands to itself (one token).
/// - A word with `*`/`?`/`[` is globbed via `fs.glob`. If the glob matches
///   nothing, bash's default (nullglob off) is to pass the literal pattern
///   through unchanged — we match that so `ls *.none` yields `*.none`.
///
/// Globbing goes through `ctx.fs` so it sees the WORKSPACE, never the host
/// cwd (red-team AD4). Local and cloud back-ends both honor this seam.
pub async fn expand_glob(fs: &dyn Filesystem, word: &str) -> Vec<String> {
    if !has_glob_meta(word) {
        return vec![word.to_string()];
    }
    let opts = GlobOpts {
        respect_gitignore: false,
        max_results: 10_000,
        sort: leti_core::adapters::filesystem::GlobSort::PathAsc,
    };
    match fs.glob(word, opts).await {
        Ok(paths) if !paths.is_empty() => paths
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        // No match or glob error → literal (nullglob off, bash default).
        _ => vec![word.to_string()],
    }
}

pub(super) fn has_glob_meta(s: &str) -> bool {
    s.chars().any(|c| matches!(c, '*' | '?' | '['))
}
