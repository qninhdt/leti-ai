//! Single-file / stream content readers: `cat`, `head`, `tail`, `wc`,
//! `cut`, `tr`. Each reads its file operands through `ctx.fs` (never the
//! host) and falls back to `stdin` when no file operand is given.

use std::path::Path;

use super::{fs_err_msg, BuiltinCtx, BuiltinResult};

/// `cat [-n] [file...]` — concatenate files (or stdin). `-n` numbers lines.
pub(super) async fn cat(ctx: &BuiltinCtx<'_>, argv: &[String], stdin: &str) -> BuiltinResult {
    let mut number = false;
    let mut files = Vec::new();
    for arg in &argv[1..] {
        if arg == "-n" {
            number = true;
        } else if arg == "-" {
            // POSIX: `-` means stdin. Treat it as no-op here and let the
            // stdin fallback below handle it when no real files are present.
            continue;
        } else {
            files.push(arg.clone());
        }
    }
    let input = match gather(ctx, "cat", &files, stdin).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    if !number {
        return BuiltinResult::out(input);
    }
    let mut out = String::new();
    for (i, line) in input.lines().enumerate() {
        out.push_str(&format!("{:>6}\t{line}\n", i + 1));
    }
    BuiltinResult::out(out)
}

/// `head [-n N] [file...]` — first N lines (default 10) of each input.
pub(super) async fn head(ctx: &BuiltinCtx<'_>, argv: &[String], stdin: &str) -> BuiltinResult {
    let (n, files) = match parse_count_flag(argv, "head", 10) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let input = match gather(ctx, "head", &files, stdin).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let out: String = input.lines().take(n).map(|l| format!("{l}\n")).collect();
    BuiltinResult::out(out)
}

/// `tail [-n N] [file...]` — last N lines (default 10) of each input.
pub(super) async fn tail(ctx: &BuiltinCtx<'_>, argv: &[String], stdin: &str) -> BuiltinResult {
    let (n, files) = match parse_count_flag(argv, "tail", 10) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let input = match gather(ctx, "tail", &files, stdin).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let lines: Vec<&str> = input.lines().collect();
    let start = lines.len().saturating_sub(n);
    let out: String = lines[start..].iter().map(|l| format!("{l}\n")).collect();
    BuiltinResult::out(out)
}

/// Parse a `-n N` (or `-N`) count flag shared by `head`/`tail`. Returns the
/// count plus the remaining file operands.
fn parse_count_flag(
    argv: &[String],
    name: &str,
    default: usize,
) -> Result<(usize, Vec<String>), BuiltinResult> {
    let mut n = default;
    let mut files = Vec::new();
    let mut i = 1;
    while i < argv.len() {
        let arg = &argv[i];
        if arg == "-n" {
            i += 1;
            let Some(v) = argv.get(i) else {
                return Err(BuiltinResult::err(
                    format!("{name}: option requires an argument -- 'n'"),
                    2,
                ));
            };
            n = v.parse().map_err(|_| {
                BuiltinResult::err(format!("{name}: invalid number of lines: '{v}'"), 2)
            })?;
        } else if let Some(rest) = arg.strip_prefix("-n") {
            n = rest.parse().map_err(|_| {
                BuiltinResult::err(format!("{name}: invalid number of lines: '{rest}'"), 2)
            })?;
        } else if let Some(rest) = arg.strip_prefix('-') {
            // `-5` shorthand for `-n 5`.
            if let Ok(v) = rest.parse() {
                n = v;
            } else {
                return Err(BuiltinResult::err(
                    format!("{name}: invalid option -- '{rest}'"),
                    2,
                ));
            }
        } else {
            files.push(arg.clone());
        }
        i += 1;
    }
    Ok((n, files))
}

/// `wc [-l] [-w] [-c] [file...]` — count lines / words / bytes. With no
/// flags, prints all three (lines, words, bytes) like GNU wc.
pub(super) async fn wc(ctx: &BuiltinCtx<'_>, argv: &[String], stdin: &str) -> BuiltinResult {
    let (mut lines_f, mut words_f, mut bytes_f) = (false, false, false);
    let mut files = Vec::new();
    for arg in &argv[1..] {
        if let Some(flags) = super::short_flags(arg) {
            for f in flags {
                match f {
                    'l' => lines_f = true,
                    'w' => words_f = true,
                    'c' | 'm' => bytes_f = true,
                    _ => return BuiltinResult::err(format!("wc: invalid option -- '{f}'"), 2),
                }
            }
        } else {
            files.push(arg.clone());
        }
    }
    let show_all = !(lines_f || words_f || bytes_f);
    let input = match gather(ctx, "wc", &files, stdin).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    let lc = input.lines().count();
    let wc_ = input.split_whitespace().count();
    let bc = input.len();
    let mut parts = Vec::new();
    if lines_f || show_all {
        parts.push(format!("{lc:>7}"));
    }
    if words_f || show_all {
        parts.push(format!("{wc_:>7}"));
    }
    if bytes_f || show_all {
        parts.push(format!("{bc:>7}"));
    }
    BuiltinResult::out(format!("{}\n", parts.join(" ").trim_start()))
}

/// `cut -d DELIM -f LIST [file...]` or `cut -c LIST` — select fields or
/// character ranges from each line. `LIST` is a single number or `a-b`
/// range (round-1: no comma lists).
pub(super) async fn cut(ctx: &BuiltinCtx<'_>, argv: &[String], stdin: &str) -> BuiltinResult {
    let mut delim = '\t';
    let mut fields: Option<String> = None;
    let mut chars_spec: Option<String> = None;
    let mut files = Vec::new();
    let mut i = 1;
    while i < argv.len() {
        let arg = &argv[i];
        match arg.as_str() {
            "-d" => {
                i += 1;
                let Some(v) = argv.get(i) else {
                    return BuiltinResult::err("cut: option requires an argument -- 'd'", 2);
                };
                delim = v.chars().next().unwrap_or('\t');
            }
            "-f" => {
                i += 1;
                let Some(v) = argv.get(i) else {
                    return BuiltinResult::err("cut: option requires an argument -- 'f'", 2);
                };
                fields = Some(v.clone());
            }
            "-c" => {
                i += 1;
                let Some(v) = argv.get(i) else {
                    return BuiltinResult::err("cut: option requires an argument -- 'c'", 2);
                };
                chars_spec = Some(v.clone());
            }
            _ if arg.starts_with("-d") => delim = arg[2..].chars().next().unwrap_or('\t'),
            _ if arg.starts_with("-f") => fields = Some(arg[2..].to_string()),
            _ if arg.starts_with("-c") => chars_spec = Some(arg[2..].to_string()),
            _ => files.push(arg.clone()),
        }
        i += 1;
    }

    let input = match gather(ctx, "cut", &files, stdin).await {
        Ok(s) => s,
        Err(e) => return e,
    };

    let mut out = String::new();
    if let Some(spec) = chars_spec {
        let (lo, hi) = match parse_range(&spec) {
            Some(r) => r,
            None => return BuiltinResult::err(format!("cut: invalid range: {spec}"), 2),
        };
        for line in input.lines() {
            let chars: Vec<char> = line.chars().collect();
            let end = hi.unwrap_or(chars.len()).min(chars.len());
            let start = lo.saturating_sub(1);
            let slice: String = chars.get(start..end).unwrap_or(&[]).iter().collect();
            out.push_str(&slice);
            out.push('\n');
        }
    } else if let Some(spec) = fields {
        let (lo, hi) = match parse_range(&spec) {
            Some(r) => r,
            None => return BuiltinResult::err(format!("cut: invalid field list: {spec}"), 2),
        };
        for line in input.lines() {
            let cols: Vec<&str> = line.split(delim).collect();
            let end = hi.unwrap_or(cols.len()).min(cols.len());
            let start = lo.saturating_sub(1);
            let selected = cols.get(start..end).unwrap_or(&[]);
            out.push_str(&selected.join(&delim.to_string()));
            out.push('\n');
        }
    } else {
        return BuiltinResult::err(
            "cut: you must specify a list of bytes, characters, or fields",
            2,
        );
    }
    BuiltinResult::out(out)
}

/// Parse `N` or `N-M` or `N-` or `-M` (1-indexed) into `(lo, Option<hi>)`.
fn parse_range(spec: &str) -> Option<(usize, Option<usize>)> {
    if let Some((a, b)) = spec.split_once('-') {
        let lo = if a.is_empty() { 1 } else { a.parse().ok()? };
        let hi = if b.is_empty() { None } else { Some(b.parse().ok()?) };
        Some((lo, hi))
    } else {
        let n = spec.parse().ok()?;
        Some((n, Some(n)))
    }
}

/// `tr SET1 SET2` / `tr -d SET1` — translate or delete characters. Round-1
/// handles literal character sets plus simple `a-z` ranges and `-d`.
pub(super) fn tr(argv: &[String], stdin: &str) -> BuiltinResult {
    let mut delete = false;
    let mut sets = Vec::new();
    for arg in &argv[1..] {
        if arg == "-d" {
            delete = true;
        } else {
            sets.push(arg.clone());
        }
    }
    if delete {
        let Some(set1) = sets.first() else {
            return BuiltinResult::err("tr: missing operand", 2);
        };
        let set1 = expand_tr_set(set1);
        let out: String = stdin.chars().filter(|c| !set1.contains(c)).collect();
        return BuiltinResult::out(out);
    }
    if sets.len() != 2 {
        return BuiltinResult::err("tr: usage: tr SET1 SET2 (or -d SET1)", 2);
    }
    let from = expand_tr_set(&sets[0]);
    let to = expand_tr_set(&sets[1]);
    let out: String = stdin
        .chars()
        .map(|c| match from.iter().position(|&f| f == c) {
            // GNU tr: if SET2 is shorter, the last char repeats.
            Some(idx) => *to.get(idx).or_else(|| to.last()).unwrap_or(&c),
            None => c,
        })
        .collect();
    BuiltinResult::out(out)
}

/// Expand a `tr` set, supporting simple `a-z` ranges.
fn expand_tr_set(set: &str) -> Vec<char> {
    let chars: Vec<char> = set.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if i + 2 < chars.len() && chars[i + 1] == '-' {
            let (start, end) = (chars[i], chars[i + 2]);
            if start <= end {
                for c in start..=end {
                    out.push(c);
                }
                i += 3;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Read file operands via `ctx.fs`, or return `stdin` when there are none.
async fn gather(
    ctx: &BuiltinCtx<'_>,
    name: &str,
    files: &[String],
    stdin: &str,
) -> Result<String, BuiltinResult> {
    if files.is_empty() {
        return Ok(stdin.to_string());
    }
    let mut out = String::new();
    for f in files {
        match ctx.fs.read(Path::new(f), None).await {
            Ok(bytes) => out.push_str(&String::from_utf8_lossy(&bytes)),
            Err(e) => return Err(BuiltinResult::err(format!("{name}: {}", fs_err_msg(&e)), 1)),
        }
    }
    Ok(out)
}
