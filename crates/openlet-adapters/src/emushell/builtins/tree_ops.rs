//! Directory + content search: `ls`, `find`, `grep`. Each pushes the work
//! down to `ctx.fs` (`list` / `glob` / `grep`) rather than walking the host
//! filesystem in-process — the search never leaves the workspace seam.

use std::path::{Path, PathBuf};

use openlet_core::adapters::filesystem::{GlobOpts, GlobSort, GrepArgs};

use super::{fs_err_msg, short_flags, BuiltinCtx, BuiltinResult};

/// `ls [-l] [-a] [-1] [path...]` — list directory children via `ctx.fs.list`.
/// The long form (`-l`) prints a `type size name` row; otherwise names only.
pub(super) async fn ls(ctx: &BuiltinCtx<'_>, argv: &[String]) -> BuiltinResult {
    let (mut long, mut all) = (false, false);
    let mut paths = Vec::new();
    for arg in &argv[1..] {
        if let Some(flags) = short_flags(arg) {
            for f in flags {
                match f {
                    'l' => long = true,
                    'a' => all = true,
                    '1' => long = false,
                    _ => return BuiltinResult::err(format!("ls: invalid option -- '{f}'"), 2),
                }
            }
        } else {
            paths.push(arg.clone());
        }
    }
    if paths.is_empty() {
        paths.push(".".to_string());
    }

    let mut out = String::new();
    let multi = paths.len() > 1;
    for (i, p) in paths.iter().enumerate() {
        match ctx.fs.list(Path::new(p)).await {
            Ok(entries) => {
                if multi {
                    if i > 0 {
                        out.push('\n');
                    }
                    out.push_str(&format!("{p}:\n"));
                }
                for e in entries {
                    // Skip dotfiles unless `-a`, matching `ls` default.
                    if !all && e.name.starts_with('.') {
                        continue;
                    }
                    if long {
                        let kind = if e.is_dir { "d" } else { "-" };
                        let size = e.size.unwrap_or(0);
                        out.push_str(&format!("{kind} {size:>8} {}\n", e.name));
                    } else {
                        out.push_str(&e.name);
                        out.push('\n');
                    }
                }
            }
            Err(e) => {
                return BuiltinResult::err(format!("ls: {}", fs_err_msg(&e)), 1);
            }
        }
    }
    BuiltinResult::out(out)
}

/// `find [path] [-name PATTERN] [-type f|d]` — enumerate matching paths via
/// `ctx.fs.glob`. `-name` becomes a `**/PATTERN` glob rooted at `path`;
/// `-type` filters files vs directories.
pub(super) async fn find(ctx: &BuiltinCtx<'_>, argv: &[String]) -> BuiltinResult {
    let mut root = ".".to_string();
    let mut name: Option<String> = None;
    let mut type_filter: Option<char> = None;
    let mut root_seen = false;
    let mut i = 1;
    while i < argv.len() {
        let arg = &argv[i];
        match arg.as_str() {
            "-name" => {
                i += 1;
                let Some(v) = argv.get(i) else {
                    return BuiltinResult::err("find: -name requires an argument", 2);
                };
                name = Some(v.clone());
            }
            "-type" => {
                i += 1;
                let Some(v) = argv.get(i) else {
                    return BuiltinResult::err("find: -type requires an argument", 2);
                };
                type_filter = v.chars().next();
            }
            _ if arg.starts_with('-') => {
                return BuiltinResult::err(format!("find: unsupported predicate: {arg}"), 2);
            }
            _ if !root_seen => {
                root = arg.clone();
                root_seen = true;
            }
            _ => return BuiltinResult::err(format!("find: extra operand: {arg}"), 2),
        }
        i += 1;
    }

    // Build a glob rooted at `root`. `-name '*.txt'` under `src` becomes
    // `src/**/*.txt`; with no `-name`, list everything under root.
    let root_norm = root.trim_end_matches('/');
    let root_prefix = if root_norm == "." || root_norm.is_empty() {
        String::new()
    } else {
        format!("{root_norm}/")
    };
    let pattern = match &name {
        Some(p) => format!("{root_prefix}**/{p}"),
        None => format!("{root_prefix}**/*"),
    };

    let opts = GlobOpts {
        respect_gitignore: false,
        max_results: 10_000,
        sort: GlobSort::PathAsc,
    };
    let matches = match ctx.fs.glob(&pattern, opts).await {
        Ok(m) => m,
        Err(e) => return BuiltinResult::err(format!("find: {}", fs_err_msg(&e)), 1),
    };

    // `glob` only returns files. `-type d` would need directory entries the
    // glob seam does not surface; reject it loudly rather than lie.
    if type_filter == Some('d') {
        return BuiltinResult::err("find: -type d not supported (glob returns files only)", 2);
    }

    let mut out = String::new();
    for m in matches {
        // Prefix `./` when the user gave no explicit root, matching `find .`.
        if root_norm == "." || root_norm.is_empty() {
            out.push_str(&format!("./{}\n", m.display()));
        } else {
            out.push_str(&format!("{}\n", m.display()));
        }
    }
    BuiltinResult::out(out)
}

/// `grep [-r] [-n] [-i] [-l] PATTERN [path...]` — content search. Pushes the
/// regex down to `ctx.fs.grep` (RE2, linear-time) rather than scanning files
/// in-process. When given piped `stdin` and no path, filters that instead.
pub(super) async fn grep(ctx: &BuiltinCtx<'_>, argv: &[String], stdin: &str) -> BuiltinResult {
    let (mut recursive, mut number, mut ignore_case, mut files_only) = (false, false, false, false);
    let mut positional = Vec::new();
    for arg in &argv[1..] {
        if let Some(flags) = short_flags(arg) {
            for f in flags {
                match f {
                    'r' | 'R' => recursive = true,
                    'n' => number = true,
                    'i' => ignore_case = true,
                    'l' => files_only = true,
                    _ => return BuiltinResult::err(format!("grep: invalid option -- '{f}'"), 2),
                }
            }
        } else {
            positional.push(arg.clone());
        }
    }
    let Some((pattern, paths)) = positional.split_first() else {
        return BuiltinResult::err("grep: no pattern given", 2);
    };

    // No path operand + piped stdin → filter stdin line-by-line in-process
    // (this is a stream filter, not a workspace walk).
    if paths.is_empty() && !recursive {
        return grep_stdin(pattern, stdin, number, ignore_case, files_only);
    }

    let path_glob = grep_path_glob(paths, recursive);
    let args = GrepArgs {
        pattern: pattern.clone(),
        path_glob,
        case_insensitive: ignore_case,
        ..GrepArgs::default()
    };
    let hits = match ctx.fs.grep(args).await {
        Ok(h) => h,
        Err(e) => return BuiltinResult::err(format!("grep: {}", fs_err_msg(&e)), 2),
    };
    if hits.is_empty() {
        // grep exit 1 = no match (not an error).
        return BuiltinResult {
            stdout: String::new(),
            stderr: String::new(),
            status: 1,
        };
    }

    let mut out = String::new();
    if files_only {
        let mut seen: Vec<PathBuf> = Vec::new();
        for h in hits {
            if !seen.contains(&h.path) {
                out.push_str(&format!("{}\n", h.path.display()));
                seen.push(h.path);
            }
        }
    } else {
        for h in hits {
            if number {
                out.push_str(&format!("{}:{}:{}\n", h.path.display(), h.line, h.text));
            } else {
                out.push_str(&format!("{}:{}\n", h.path.display(), h.text));
            }
        }
    }
    BuiltinResult::out(out)
}

/// Derive the `path_glob` for a `ctx.fs.grep` call from grep's path operands.
/// A directory operand (or `-r`) searches everything under it; a single file
/// operand narrows to that exact path.
fn grep_path_glob(paths: &[String], recursive: bool) -> Option<String> {
    match paths.first() {
        None => None,
        Some(p) if p == "." => None,
        Some(p) => {
            let norm = p.trim_end_matches('/');
            if recursive {
                Some(format!("{norm}/**/*"))
            } else {
                Some(norm.to_string())
            }
        }
    }
}

/// Filter piped `stdin` line-by-line — the non-recursive `cmd | grep x` case.
fn grep_stdin(
    pattern: &str,
    stdin: &str,
    number: bool,
    ignore_case: bool,
    files_only: bool,
) -> BuiltinResult {
    let re = match regex::RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .build()
    {
        Ok(r) => r,
        Err(e) => return BuiltinResult::err(format!("grep: bad regex: {e}"), 2),
    };
    let mut out = String::new();
    let mut matched = false;
    for (idx, line) in stdin.lines().enumerate() {
        if re.is_match(line) {
            matched = true;
            if files_only {
                // No filename for stdin; `-l` on stdin prints `(standard input)`.
                out = "(standard input)\n".to_string();
                break;
            }
            if number {
                out.push_str(&format!("{}:{line}\n", idx + 1));
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    if matched {
        BuiltinResult::out(out)
    } else {
        BuiltinResult {
            stdout: String::new(),
            stderr: String::new(),
            status: 1,
        }
    }
}
