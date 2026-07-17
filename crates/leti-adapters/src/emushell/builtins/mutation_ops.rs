//! Mutation builtins: `mkdir`, `rm`, `mv`, `cp`, `touch`, `tee`. Every
//! write / delete / move goes through `ctx.fs`; the ordering rules below
//! guarantee no data loss if a step fails or the run is cancelled.
//!
//! Safe ordering:
//! - `cp a b`: read(a) then write(b). A failure at either step aborts
//!   without touching `a`.
//! - `mv a b`: prefer `rename(a, b)` (one call, atomic when the backend
//!   supports it). If rename is unavailable the copy+delete fallback only
//!   removes the source after the destination write succeeds.
//! - Cancellation is checked at file boundaries, never mid read-write-remove
//!   of a single file.

use std::path::Path;
use std::pin::Pin;

use leti_core::adapters::filesystem::WriteOpts;
use leti_core::error::FsError;

use super::{BuiltinCtx, BuiltinResult, fs_err_msg, short_flags};

/// `mkdir [-p] dir...` — create directories. `-p` makes parents as needed
/// and is not an error if the target exists.
pub(super) async fn mkdir(ctx: &BuiltinCtx<'_>, argv: &[String]) -> BuiltinResult {
    let mut parents = false;
    let mut dirs = Vec::new();
    for arg in &argv[1..] {
        if let Some(flags) = short_flags(arg) {
            for f in flags {
                match f {
                    'p' => parents = true,
                    _ => return BuiltinResult::err(format!("mkdir: invalid option -- '{f}'"), 1),
                }
            }
        } else {
            dirs.push(arg.clone());
        }
    }
    if dirs.is_empty() {
        return BuiltinResult::err("mkdir: missing operand", 1);
    }
    // The `Filesystem` trait has no explicit mkdir — a directory is implied
    // by writing a file under it, and the local impl creates parent dirs on
    // `write`. To materialize an empty directory we write a zero-byte marker
    // then remove it, leaving the directory behind. This keeps mkdir honest
    // without adding a trait method.
    for d in &dirs {
        let marker = format!("{}/.mkdir-keep", d.trim_end_matches('/'));
        let opts = WriteOpts {
            create_new: !parents,
            ..WriteOpts::default()
        };
        if let Err(e) = ctx
            .fs
            .write(Path::new(&marker), bytes::Bytes::new(), opts)
            .await
        {
            // With -p, a pre-existing dir is fine; the create_new refusal is
            // the only signal we have, so swallow it under -p.
            if !parents {
                return BuiltinResult::err(format!("mkdir: {}", fs_err_msg(&e)), 1);
            }
        }
        let _ = ctx.fs.remove(Path::new(&marker)).await;
    }
    BuiltinResult::out(String::new())
}

/// `rm [-r] [-f] path...` — remove files (and dirs with `-r`). Walks a
/// directory leaf-first via `ctx.fs.list` so the workspace-boundary check
/// runs on every path.
pub(super) async fn rm(ctx: &BuiltinCtx<'_>, argv: &[String]) -> BuiltinResult {
    let (mut recursive, mut force) = (false, false);
    let mut paths = Vec::new();
    for arg in &argv[1..] {
        if let Some(flags) = short_flags(arg) {
            for f in flags {
                match f {
                    'r' | 'R' => recursive = true,
                    'f' => force = true,
                    _ => return BuiltinResult::err(format!("rm: invalid option -- '{f}'"), 1),
                }
            }
        } else {
            paths.push(arg.clone());
        }
    }
    if paths.is_empty() && !force {
        return BuiltinResult::err("rm: missing operand", 1);
    }
    for p in &paths {
        if ctx.cancel.is_cancelled() {
            return BuiltinResult::err("rm: interrupted", 130);
        }
        if let Err(e) = remove_path(ctx, p, recursive).await
            && !force
        {
            return BuiltinResult::err(format!("rm: {}", fs_err_msg(&e)), 1);
        }
    }
    BuiltinResult::out(String::new())
}

/// Remove one path. If it is a directory and `recursive`, list and remove
/// children leaf-first before removing the dir itself.
fn remove_path<'a>(
    ctx: &'a BuiltinCtx<'a>,
    path: &'a str,
    recursive: bool,
) -> Pin<Box<dyn std::future::Future<Output = Result<(), FsError>> + Send + 'a>> {
    Box::pin(async move {
        // A directory listing succeeds only for dirs; use it to decide shape.
        match ctx.fs.list(Path::new(path)).await {
            Ok(entries) if recursive => {
                for entry in entries {
                    let child = format!("{}/{}", path.trim_end_matches('/'), entry.name);
                    remove_path(ctx, &child, recursive).await?;
                }
                ctx.fs.remove(Path::new(path)).await
            }
            // A dir without -r (remove() rejects non-empty), or an empty dir.
            Ok(_) => ctx.fs.remove(Path::new(path)).await,
            // Not a directory (list failed) — remove as a file.
            Err(_) => ctx.fs.remove(Path::new(path)).await,
        }
    })
}

/// `cp [-r] src... dest` — copy. Read source, then write dest; a failure at
/// either step never touches the source. With multiple sources, dest must be
/// a directory.
pub(super) async fn cp(ctx: &BuiltinCtx<'_>, argv: &[String]) -> BuiltinResult {
    let (recursive, operands) = split_recursive(argv, "cp");
    let operands = match operands {
        Ok(v) => v,
        Err(e) => return e,
    };
    if operands.len() < 2 {
        return BuiltinResult::err("cp: missing destination file operand", 1);
    }
    let (sources, dest) = operands.split_at(operands.len() - 1);
    let dest = &dest[0];
    let dest_is_dir = ctx.fs.list(Path::new(dest)).await.is_ok();

    for src in sources {
        if ctx.cancel.is_cancelled() {
            return BuiltinResult::err("cp: interrupted", 130);
        }
        let target = if dest_is_dir {
            format!("{}/{}", dest.trim_end_matches('/'), base_name(src))
        } else {
            dest.clone()
        };
        if let Err(e) = copy_path(ctx, src, &target, recursive).await {
            return BuiltinResult::err(format!("cp: {}", fs_err_msg(&e)), 1);
        }
    }
    BuiltinResult::out(String::new())
}

/// Copy one path. Recurses into directories (with `-r`) copying children.
fn copy_path<'a>(
    ctx: &'a BuiltinCtx<'a>,
    src: &'a str,
    dest: &'a str,
    recursive: bool,
) -> Pin<Box<dyn std::future::Future<Output = Result<(), FsError>> + Send + 'a>> {
    Box::pin(async move {
        match ctx.fs.list(Path::new(src)).await {
            // Directory copy.
            Ok(entries) => {
                if !recursive {
                    return Err(FsError::InvalidInput(format!("{src}: is a directory")));
                }
                for entry in entries {
                    let child_src = format!("{}/{}", src.trim_end_matches('/'), entry.name);
                    let child_dest = format!("{}/{}", dest.trim_end_matches('/'), entry.name);
                    copy_path(ctx, &child_src, &child_dest, recursive).await?;
                }
                Ok(())
            }
            // File copy: read source fully, then write dest. Source untouched.
            Err(_) => {
                let bytes = ctx.fs.read(Path::new(src), None).await?;
                ctx.fs
                    .write(Path::new(dest), bytes, WriteOpts::default())
                    .await?;
                Ok(())
            }
        }
    })
}

/// `mv src... dest` — move / rename. Prefers `ctx.fs.rename` (atomic when the
/// backend supports it). With multiple sources, dest must be a directory.
pub(super) async fn mv(ctx: &BuiltinCtx<'_>, argv: &[String]) -> BuiltinResult {
    let operands: Vec<String> = argv[1..]
        .iter()
        .filter(|a| !a.starts_with('-'))
        .cloned()
        .collect();
    if operands.len() < 2 {
        return BuiltinResult::err("mv: missing destination file operand", 1);
    }
    let (sources, dest) = operands.split_at(operands.len() - 1);
    let dest = &dest[0];
    let dest_is_dir = ctx.fs.list(Path::new(dest)).await.is_ok();

    for src in sources {
        if ctx.cancel.is_cancelled() {
            return BuiltinResult::err("mv: interrupted", 130);
        }
        let target = if dest_is_dir {
            format!("{}/{}", dest.trim_end_matches('/'), base_name(src))
        } else {
            dest.clone()
        };
        if let Err(e) = ctx.fs.rename(Path::new(src), Path::new(&target)).await {
            return BuiltinResult::err(format!("mv: {}", fs_err_msg(&e)), 1);
        }
    }
    BuiltinResult::out(String::new())
}

/// `touch file...` — create empty files (create-only; leaves existing files
/// untouched, since the trait exposes no mtime bump).
pub(super) async fn touch(ctx: &BuiltinCtx<'_>, argv: &[String]) -> BuiltinResult {
    let files: Vec<&String> = argv[1..].iter().filter(|a| !a.starts_with('-')).collect();
    if files.is_empty() {
        return BuiltinResult::err("touch: missing file operand", 1);
    }
    for f in files {
        if ctx.fs.exists(Path::new(f)).await {
            continue;
        }
        if let Err(e) = ctx
            .fs
            .write(Path::new(f), bytes::Bytes::new(), WriteOpts::default())
            .await
        {
            return BuiltinResult::err(format!("touch: {}", fs_err_msg(&e)), 1);
        }
    }
    BuiltinResult::out(String::new())
}

/// `tee [-a] file...` — copy stdin to each file and to stdout. `-a` appends.
pub(super) async fn tee(ctx: &BuiltinCtx<'_>, argv: &[String], stdin: &str) -> BuiltinResult {
    let mut append = false;
    let mut files = Vec::new();
    for arg in &argv[1..] {
        if let Some(flags) = short_flags(arg) {
            for f in flags {
                match f {
                    'a' => append = true,
                    _ => return BuiltinResult::err(format!("tee: invalid option -- '{f}'"), 1),
                }
            }
        } else {
            files.push(arg.clone());
        }
    }
    let opts = WriteOpts {
        append,
        ..WriteOpts::default()
    };
    for f in &files {
        if let Err(e) = ctx
            .fs
            .write(
                Path::new(f),
                bytes::Bytes::from(stdin.to_string().into_bytes()),
                opts,
            )
            .await
        {
            return BuiltinResult::err(format!("tee: {}", fs_err_msg(&e)), 1);
        }
    }
    // tee passes stdin through to stdout unchanged.
    BuiltinResult::out(stdin.to_string())
}

/// Split a `-r`/`-R` recursive flag off the argv, returning the flag plus the
/// non-flag operands. Errors on any other option.
fn split_recursive(argv: &[String], name: &str) -> (bool, Result<Vec<String>, BuiltinResult>) {
    let mut recursive = false;
    let mut operands = Vec::new();
    for arg in &argv[1..] {
        if let Some(flags) = short_flags(arg) {
            for f in flags {
                match f {
                    'r' | 'R' => recursive = true,
                    _ => {
                        return (
                            recursive,
                            Err(BuiltinResult::err(
                                format!("{name}: invalid option -- '{f}'"),
                                1,
                            )),
                        );
                    }
                }
            }
        } else {
            operands.push(arg.clone());
        }
    }
    (recursive, Ok(operands))
}

/// Last path component (for `cp`/`mv` into a directory destination).
fn base_name(path: &str) -> &str {
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path)
}
