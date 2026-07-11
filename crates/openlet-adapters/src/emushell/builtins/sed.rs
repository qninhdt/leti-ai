//! The `sed` 80/20 subset.
//!
//! Deliberately narrow — the common LLM idioms only. Anything outside the
//! enumerated subset fails loud with `unsupported` rather than silently
//! producing wrong output.
//!
//! `sed` subset: `s/re/repl/[gI]` substitution, `-n` + `p` print, `-i`
//! in-place. Line addresses (`1,5s`), hold space, and branches are not
//! supported.

use std::path::Path;

use regex::RegexBuilder;

use super::{BuiltinCtx, BuiltinResult, fs_err_msg, gather};

/// `sed [-n] [-i] 's/re/repl/flags' [file...]`.
pub(super) async fn sed(ctx: &BuiltinCtx<'_>, argv: &[String], stdin: &str) -> BuiltinResult {
    let mut quiet = false;
    let mut in_place = false;
    let mut script: Option<String> = None;
    let mut files = Vec::new();
    for arg in &argv[1..] {
        match arg.as_str() {
            "-n" => quiet = true,
            "-i" => in_place = true,
            _ if arg.starts_with('-') && arg.len() > 1 => {
                return BuiltinResult::err(format!("sed: unsupported option: {arg}"), 2);
            }
            _ if script.is_none() => script = Some(arg.clone()),
            _ => files.push(arg.clone()),
        }
    }
    let Some(script) = script else {
        return BuiltinResult::err("sed: no script given", 2);
    };

    let program = match parse_sed_script(&script) {
        Ok(p) => p,
        Err(e) => return BuiltinResult::err(e, 2),
    };

    // In-place edit rewrites each file through `ctx.fs`; otherwise stream.
    if in_place {
        if files.is_empty() {
            return BuiltinResult::err("sed: -i requires a file operand", 2);
        }
        for f in &files {
            let content = match ctx.fs.read(Path::new(f), None).await {
                Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                Err(e) => return BuiltinResult::err(format!("sed: {}", fs_err_msg(&e)), 2),
            };
            let transformed = program.apply(&content, quiet);
            let opts = openlet_core::adapters::filesystem::WriteOpts::default();
            if let Err(e) = ctx
                .fs
                .write(
                    Path::new(f),
                    bytes::Bytes::from(transformed.into_bytes()),
                    opts,
                )
                .await
            {
                return BuiltinResult::err(format!("sed: {}", fs_err_msg(&e)), 2);
            }
        }
        return BuiltinResult::out(String::new());
    }

    let input = match gather(ctx, "sed", &files, stdin).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    BuiltinResult::out(program.apply(&input, quiet))
}

/// A parsed `sed` program — round-1 is a single substitution plus an
/// optional `p` (print) command driven by `-n`.
struct SedProgram {
    re: regex::Regex,
    repl: String,
    global: bool,
    /// `p` command present — print matching lines (meaningful under `-n`).
    print: bool,
}

impl SedProgram {
    fn apply(&self, input: &str, quiet: bool) -> String {
        let mut out = String::new();
        for line in input.lines() {
            let replaced = if self.global {
                self.re.replace_all(line, self.repl.as_str())
            } else {
                self.re.replace(line, self.repl.as_str())
            };
            // Under `-n`, nothing prints unless a `p` command matched a line
            // that actually changed (approximation of sed's auto-print off).
            if quiet {
                if self.print && self.re.is_match(line) {
                    out.push_str(&replaced);
                    out.push('\n');
                }
            } else {
                out.push_str(&replaced);
                out.push('\n');
            }
        }
        out
    }
}

/// Parse `s/re/repl/flags` (optionally trailed by `;p`). Only `s` and a
/// bare `p` are recognized.
fn parse_sed_script(script: &str) -> Result<SedProgram, String> {
    let mut print = false;
    // Allow a trailing `;p` or `p` appended to the substitution.
    let core = script.trim();
    let (subst, tail) = match core.rsplit_once(';') {
        Some((s, t)) if t.trim() == "p" => {
            print = true;
            (s.trim(), "")
        }
        _ => (core, ""),
    };
    let _ = tail;

    if !subst.starts_with('s') {
        return Err(format!(
            "sed: unsupported command (only s/// supported): {script}"
        ));
    }
    // Delimiter is the char right after `s` (usually `/`).
    let bytes = subst.as_bytes();
    if bytes.len() < 2 {
        return Err("sed: malformed s/// command".into());
    }
    let delim = bytes[1] as char;
    let rest = &subst[2..];
    let parts: Vec<&str> = split_unescaped(rest, delim);
    if parts.len() < 2 {
        return Err(format!(
            "sed: malformed s{delim}re{delim}repl{delim} command"
        ));
    }
    let pattern = parts[0];
    let repl = unescape_repl(parts[1]);
    let flags = parts.get(2).copied().unwrap_or("");
    let mut global = false;
    let mut case_insensitive = false;
    for c in flags.chars() {
        match c {
            'g' => global = true,
            'I' | 'i' => case_insensitive = true,
            'p' => print = true,
            _ => return Err(format!("sed: unsupported flag: {c}")),
        }
    }
    let re = RegexBuilder::new(pattern)
        .case_insensitive(case_insensitive)
        .build()
        .map_err(|e| format!("sed: bad regex: {e}"))?;
    Ok(SedProgram {
        re,
        repl,
        global,
        print,
    })
}

/// Split on an unescaped delimiter (a `\` before the delimiter escapes it).
fn split_unescaped(s: &str, delim: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut prev_backslash = false;
    for (i, c) in s.char_indices() {
        if c == delim && !prev_backslash {
            parts.push(&s[start..i]);
            start = i + c.len_utf8();
        }
        prev_backslash = c == '\\' && !prev_backslash;
    }
    parts.push(&s[start..]);
    parts
}

/// Turn sed replacement escapes into their literal chars (`\/` → `/`,
/// `\n` → newline, `\t` → tab). `&` (whole-match) is not supported round-1
/// and passes through literally.
fn unescape_repl(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}
