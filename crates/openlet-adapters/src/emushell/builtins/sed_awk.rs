//! The `sed` / `awk` 80/20 subsets.
//!
//! Deliberately narrow — the common LLM idioms only. Anything outside the
//! enumerated subset fails loud with `unsupported` rather than silently
//! producing wrong output.
//!
//! `sed` subset: `s/re/repl/[gI]` substitution, `-n` + `p` print, `-i`
//! in-place. Line addresses (`1,5s`), hold space, and branches are not
//! supported.
//!
//! `awk` subset: `{print $N}` / `{print}`, `-F sep`, `NR` / `NF`, and
//! `BEGIN` / `END` blocks with `print`. Associative arrays, user functions,
//! and control-flow are not supported.

use std::path::Path;

use regex::RegexBuilder;

use super::{fs_err_msg, BuiltinCtx, BuiltinResult};

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
                .write(Path::new(f), bytes::Bytes::from(transformed.into_bytes()), opts)
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
        return Err(format!("sed: unsupported command (only s/// supported): {script}"));
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
        return Err(format!("sed: malformed s{delim}re{delim}repl{delim} command"));
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

/// `awk [-F sep] 'program' [file...]`. Program is one of: `{print}`,
/// `{print $N}`, `{print $N, $M}`, or `BEGIN{...}`/`END{...}` wrapping those.
pub(super) async fn awk(ctx: &BuiltinCtx<'_>, argv: &[String], stdin: &str) -> BuiltinResult {
    let mut sep: Option<char> = None;
    let mut program: Option<String> = None;
    let mut files = Vec::new();
    let mut i = 1;
    while i < argv.len() {
        let arg = &argv[i];
        match arg.as_str() {
            "-F" => {
                i += 1;
                let Some(v) = argv.get(i) else {
                    return BuiltinResult::err("awk: -F requires an argument", 2);
                };
                sep = v.chars().next();
            }
            _ if arg.starts_with("-F") => sep = arg[2..].chars().next(),
            _ if arg.starts_with('-') && arg.len() > 1 => {
                return BuiltinResult::err(format!("awk: unsupported option: {arg}"), 2);
            }
            _ if program.is_none() => program = Some(arg.clone()),
            _ => files.push(arg.clone()),
        }
        i += 1;
    }
    let Some(program) = program else {
        return BuiltinResult::err("awk: no program given", 2);
    };
    let prog = match parse_awk_program(&program) {
        Ok(p) => p,
        Err(e) => return BuiltinResult::err(e, 2),
    };
    let input = match gather(ctx, "awk", &files, stdin).await {
        Ok(s) => s,
        Err(e) => return e,
    };
    BuiltinResult::out(prog.run(&input, sep))
}

/// A parsed awk program: optional BEGIN/END print actions plus the main
/// per-line action. Round-1 supports only `print` statements.
struct AwkProgram {
    begin: Option<PrintAction>,
    main: Option<PrintAction>,
    end: Option<PrintAction>,
}

/// A `print` action: the list of field specs to emit. Empty list = `print`
/// (the whole record). A spec is either a field index (`$N`, 0 = `$0`) or a
/// literal built-in (`NR`, `NF`).
#[derive(Clone)]
struct PrintAction {
    specs: Vec<PrintSpec>,
}

#[derive(Clone)]
enum PrintSpec {
    WholeLine,
    Field(usize),
    Nr,
    Nf,
}

impl AwkProgram {
    fn run(&self, input: &str, sep: Option<char>) -> String {
        let mut out = String::new();
        if let Some(a) = &self.begin {
            a.emit(&mut out, "", &[], 0);
        }
        if let Some(main) = &self.main {
            for (idx, line) in input.lines().enumerate() {
                let fields: Vec<&str> = match sep {
                    Some(c) => line.split(c).collect(),
                    None => line.split_whitespace().collect(),
                };
                main.emit(&mut out, line, &fields, idx as u64 + 1);
            }
        }
        if let Some(a) = &self.end {
            let nr = input.lines().count() as u64;
            a.emit(&mut out, "", &[], nr);
        }
        out
    }
}

impl PrintAction {
    fn emit(&self, out: &mut String, whole: &str, fields: &[&str], nr: u64) {
        if self.specs.is_empty() {
            out.push_str(whole);
            out.push('\n');
            return;
        }
        let mut parts = Vec::new();
        for spec in &self.specs {
            let s = match spec {
                PrintSpec::WholeLine => whole.to_string(),
                PrintSpec::Field(n) => fields.get(n.saturating_sub(1)).unwrap_or(&"").to_string(),
                PrintSpec::Nr => nr.to_string(),
                PrintSpec::Nf => fields.len().to_string(),
            };
            parts.push(s);
        }
        out.push_str(&parts.join(" "));
        out.push('\n');
    }
}

/// Parse the awk subset: `BEGIN{print ...}`, `END{print ...}`, and a bare
/// `{print ...}` main block (or `print ...` with no braces).
fn parse_awk_program(program: &str) -> Result<AwkProgram, String> {
    let mut begin = None;
    let mut main = None;
    let mut end = None;
    let trimmed = program.trim();

    // Extract `BEGIN{...}` / `END{...}` blocks, leaving the main block.
    let mut rest = trimmed.to_string();
    if let Some(action) = extract_block(&mut rest, "BEGIN")? {
        begin = Some(action);
    }
    if let Some(action) = extract_block(&mut rest, "END")? {
        end = Some(action);
    }
    let rest = rest.trim();
    if !rest.is_empty() {
        let body = rest.trim().trim_start_matches('{').trim_end_matches('}').trim();
        main = Some(parse_print(body)?);
    }
    if begin.is_none() && main.is_none() && end.is_none() {
        return Err(format!("awk: unsupported program: {program}"));
    }
    Ok(AwkProgram { begin, main, end })
}

/// Pull a `KEYWORD{...}` block out of `src`, returning its parsed print
/// action and removing it from `src`. All offsets are byte indices into the
/// original string so `replace_range` cuts exactly the `KEYWORD ... }` span.
fn extract_block(src: &mut String, keyword: &str) -> Result<Option<PrintAction>, String> {
    let Some(pos) = src.find(keyword) else {
        return Ok(None);
    };
    // Byte offset of the first non-space char after the keyword.
    let after_kw = pos + keyword.len();
    let ws = src[after_kw..].len() - src[after_kw..].trim_start().len();
    let brace_open = after_kw + ws;
    if !src[brace_open..].starts_with('{') {
        return Err(format!("awk: expected {{ after {keyword}"));
    }
    let Some(rel_close) = src[brace_open..].find('}') else {
        return Err(format!("awk: unclosed {keyword} block"));
    };
    let brace_close = brace_open + rel_close;
    let body = src[brace_open + 1..brace_close].trim().to_string();
    let action = parse_print(&body)?;
    src.replace_range(pos..brace_close + 1, "");
    Ok(Some(action))
}

/// Parse a `print ...` statement body into a `PrintAction`.
fn parse_print(body: &str) -> Result<PrintAction, String> {
    let body = body.trim();
    if body.is_empty() {
        return Err("awk: empty action".into());
    }
    let rest = body
        .strip_prefix("print")
        .ok_or_else(|| format!("awk: only print supported: {body}"))?
        .trim();
    if rest.is_empty() {
        return Ok(PrintAction { specs: Vec::new() });
    }
    let mut specs = Vec::new();
    for tok in rest.split(',') {
        let tok = tok.trim();
        specs.push(parse_spec(tok)?);
    }
    Ok(PrintAction { specs })
}

fn parse_spec(tok: &str) -> Result<PrintSpec, String> {
    match tok {
        "NR" => Ok(PrintSpec::Nr),
        "NF" => Ok(PrintSpec::Nf),
        "$0" => Ok(PrintSpec::WholeLine),
        _ if tok.starts_with('$') => {
            let n = tok[1..]
                .parse::<usize>()
                .map_err(|_| format!("awk: bad field spec: {tok}"))?;
            Ok(PrintSpec::Field(n))
        }
        _ => Err(format!("awk: unsupported print token: {tok}")),
    }
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
