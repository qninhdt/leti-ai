//! The `awk` 80/20 subset.
//!
//! Deliberately narrow — the common LLM idioms only. Anything outside the
//! enumerated subset fails loud with `unsupported` rather than silently
//! producing wrong output.
//!
//! `awk` subset: `{print $N}` / `{print}`, `-F sep`, `NR` / `NF`, and
//! `BEGIN` / `END` blocks with `print`. Associative arrays, user functions,
//! and control-flow are not supported.

use super::{BuiltinCtx, BuiltinResult, gather};

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
        let body = rest
            .trim()
            .trim_start_matches('{')
            .trim_end_matches('}')
            .trim();
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
