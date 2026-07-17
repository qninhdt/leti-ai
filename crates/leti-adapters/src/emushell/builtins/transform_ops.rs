//! Pure stream / argument transforms: `echo`, `sort`, `uniq`, `basename`,
//! `dirname`, `diff`, `xargs`. These operate on `stdin` and `argv` in
//! memory; only `sort`/`uniq`/`diff` reach `ctx.fs` (to read file operands).

use std::collections::BTreeSet;

use super::{BuiltinCtx, BuiltinResult, fs_err_msg, gather, short_flags};
use std::path::Path;

/// `echo [-n] args...` — join args with spaces. `-n` suppresses the
/// trailing newline.
pub(super) fn echo(argv: &[String]) -> BuiltinResult {
    let mut newline = true;
    let mut start = 1;
    if argv.get(1).map(String::as_str) == Some("-n") {
        newline = false;
        start = 2;
    }
    let mut s = argv[start..].join(" ");
    if newline {
        s.push('\n');
    }
    BuiltinResult::out(s)
}

/// `sort [-r] [-n] [-u] [file...]` — sort lines. Flags: reverse, numeric,
/// unique. Reads file operands through `ctx.fs`, else sorts stdin.
pub(super) async fn sort(ctx: &BuiltinCtx<'_>, argv: &[String], stdin: &str) -> BuiltinResult {
    let (mut reverse, mut numeric, mut unique) = (false, false, false);
    let mut files = Vec::new();
    for arg in &argv[1..] {
        if let Some(flags) = short_flags(arg) {
            for f in flags {
                match f {
                    'r' => reverse = true,
                    'n' => numeric = true,
                    'u' => unique = true,
                    _ => return BuiltinResult::err(format!("sort: invalid option -- '{f}'"), 2),
                }
            }
        } else {
            files.push(arg.clone());
        }
    }
    let input = match gather(ctx, "sort", &files, stdin).await {
        Ok(s) => s,
        Err(e) => return e,
    };

    let mut lines: Vec<&str> = input.lines().collect();
    if numeric {
        lines.sort_by(|a, b| {
            let (na, nb) = (parse_leading_num(a), parse_leading_num(b));
            na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal)
        });
    } else {
        lines.sort_unstable();
    }
    if reverse {
        lines.reverse();
    }
    if unique {
        lines.dedup();
    }
    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    BuiltinResult::out(out)
}

/// Parse the leading number of a line for `sort -n`. Non-numeric lines sort
/// as 0, matching GNU sort's "leading numeric string" behavior loosely.
fn parse_leading_num(s: &str) -> f64 {
    let t = s.trim_start();
    let end = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(t.len());
    t[..end].parse().unwrap_or(0.0)
}

/// `uniq [-c] [-d] [-u] [file]` — collapse *adjacent* duplicate lines.
/// `-c` prefixes counts, `-d` shows only duplicated lines, `-u` only
/// unique ones.
pub(super) async fn uniq(ctx: &BuiltinCtx<'_>, argv: &[String], stdin: &str) -> BuiltinResult {
    let (mut count, mut only_dup, mut only_uniq) = (false, false, false);
    let mut files = Vec::new();
    for arg in &argv[1..] {
        if let Some(flags) = short_flags(arg) {
            for f in flags {
                match f {
                    'c' => count = true,
                    'd' => only_dup = true,
                    'u' => only_uniq = true,
                    _ => return BuiltinResult::err(format!("uniq: invalid option -- '{f}'"), 2),
                }
            }
        } else {
            files.push(arg.clone());
        }
    }
    let input = match gather(ctx, "uniq", &files, stdin).await {
        Ok(s) => s,
        Err(e) => return e,
    };

    let mut out = String::new();
    let mut iter = input.lines().peekable();
    while let Some(line) = iter.next() {
        let mut n = 1u64;
        while iter.peek() == Some(&line) {
            iter.next();
            n += 1;
        }
        let emit = if only_dup {
            n > 1
        } else if only_uniq {
            n == 1
        } else {
            true
        };
        if emit {
            if count {
                out.push_str(&format!("{n:>7} {line}\n"));
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    BuiltinResult::out(out)
}

/// `basename path [suffix]` — strip directory and optional suffix.
pub(super) fn basename(argv: &[String]) -> BuiltinResult {
    let Some(path) = argv.get(1) else {
        return BuiltinResult::err("basename: missing operand", 1);
    };
    let base = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path);
    let base = match argv.get(2) {
        Some(suffix) if base != suffix => base.strip_suffix(suffix.as_str()).unwrap_or(base),
        _ => base,
    };
    BuiltinResult::out(format!("{base}\n"))
}

/// `dirname path` — strip the last path component.
pub(super) fn dirname(argv: &[String]) -> BuiltinResult {
    let Some(path) = argv.get(1) else {
        return BuiltinResult::err("dirname: missing operand", 1);
    };
    let trimmed = path.trim_end_matches('/');
    let dir = match trimmed.rsplit_once('/') {
        Some(("", _)) => "/",
        Some((head, _)) => head,
        None => ".",
    };
    BuiltinResult::out(format!("{dir}\n"))
}

/// `diff a b` — line-level diff of two files. Emits a minimal unified-ish
/// `<`/`>` report (GNU normal format). Exit 0 = identical, 1 = differ.
pub(super) async fn diff(ctx: &BuiltinCtx<'_>, argv: &[String], _stdin: &str) -> BuiltinResult {
    let files: Vec<&String> = argv[1..].iter().filter(|a| !a.starts_with('-')).collect();
    if files.len() != 2 {
        return BuiltinResult::err("diff: usage: diff FILE1 FILE2", 2);
    }
    let a = match ctx.fs.read(Path::new(files[0]), None).await {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) => return BuiltinResult::err(format!("diff: {}", fs_err_msg(&e)), 2),
    };
    let b = match ctx.fs.read(Path::new(files[1]), None).await {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) => return BuiltinResult::err(format!("diff: {}", fs_err_msg(&e)), 2),
    };
    let la: Vec<&str> = a.lines().collect();
    let lb: Vec<&str> = b.lines().collect();
    if la == lb {
        return BuiltinResult::out(String::new());
    }
    // Minimal report: lines only in A prefixed `<`, only in B prefixed `>`.
    // Not a real LCS diff — a set-difference summary sufficient for the LLM
    // to see what changed without a diff engine (YAGNI for round-1).
    let set_a: BTreeSet<&str> = la.iter().copied().collect();
    let set_b: BTreeSet<&str> = lb.iter().copied().collect();
    let mut out = String::new();
    for line in &la {
        if !set_b.contains(line) {
            out.push_str(&format!("< {line}\n"));
        }
    }
    for line in &lb {
        if !set_a.contains(line) {
            out.push_str(&format!("> {line}\n"));
        }
    }
    BuiltinResult {
        stdout: out,
        stderr: String::new(),
        status: 1,
    }
}

/// `xargs cmd [args...]` — read whitespace-separated tokens from stdin and
/// append them as arguments to `cmd`, then dispatch it once. Does NOT
/// support `-I`, `-n`, `-0` in round-1 (fail-loud on those).
pub(super) async fn xargs(ctx: &BuiltinCtx<'_>, argv: &[String], stdin: &str) -> BuiltinResult {
    if argv.len() < 2 {
        // Bare `xargs` echoes stdin tokens joined by spaces (GNU runs
        // `/bin/echo` by default — we emulate that directly).
        let joined = stdin.split_whitespace().collect::<Vec<_>>().join(" ");
        return BuiltinResult::out(format!("{joined}\n"));
    }
    if argv[1].starts_with('-') {
        return BuiltinResult::err(format!("xargs: unsupported option: {}", argv[1]), 2);
    }
    let mut new_argv: Vec<String> = argv[1..].to_vec();
    new_argv.extend(stdin.split_whitespace().map(str::to_string));
    // Re-enter the dispatcher for the composed command. Boxing avoids an
    // infinitely-sized future from the recursive `dispatch` call.
    Box::pin(super::dispatch(ctx, &new_argv, "")).await
}
