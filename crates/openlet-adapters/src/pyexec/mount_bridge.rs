//! The single IO seam: map each Monty `OsFunctionCall` variant onto the
//! injected [`Filesystem`](openlet_core::adapters::filesystem::Filesystem).
//!
//! Monty's interpreter "never performs I/O" — it yields `RunProgress::OsCall`
//! on every filesystem/date/env op and waits for the host to resume it with a
//! value. This module is that host: one `match` arm per variant, every path op
//! routed through `ctx.fs` (async), no `std::fs`, no host disk. The exact same
//! bridge therefore works against the local FS or a cloud gRPC backend — only
//! the `Filesystem` impl differs.
//!
//! Two contracts learned from the Phase-1 spike are encoded here:
//! - **write-count (FIND-D):** `f.write(s)` in Monty reads the resume value via
//!   `as_int`, so `WriteText`/`AppendText`/`WriteBytes`/`AppendBytes` MUST resume
//!   with `Int(count)`, never `None`, or every write raises `TypeError`.
//! - **`w`-mode second write becomes `AppendText` (verified `file.rs::write`):**
//!   Monty flips a `first_write_done` flag so the FIRST write on a truncating
//!   handle emits `WriteText` and subsequent writes emit `AppendText`. We honor
//!   that by making `WriteText` truncate and `AppendText` append.

use std::path::Path;

use bytes::Bytes;
use monty::{
    ExcType, MontyDate, MontyDateTime, MontyException, MontyFileHandle, MontyObject, OsFunctionCall,
};
use openlet_core::adapters::filesystem::{Filesystem, WriteOpts};
use openlet_core::error::FsError;

/// Environment variables Python code may observe via `os.getenv` /
/// `os.environ`. Everything else — crucially `OPENAI_API_KEY` and any other
/// secret — reads back as unset. Mirrors the `localshell` scrub list but is
/// intentionally narrower: interpreted computation never needs `PATH`.
const ENV_ALLOWLIST: &[&str] = &["LANG", "LC_ALL", "LC_CTYPE", "TZ", "TERM", "USER"];

/// Outcome of dispatching one OS call: the value (or exception) to resume the
/// Monty VM with.
pub(crate) enum Dispatched {
    Ok(MontyObject),
    Err(MontyException),
}

impl Dispatched {
    fn ok(o: MontyObject) -> Self {
        Self::Ok(o)
    }
}

/// Resolve a single `OsFunctionCall` against the workspace filesystem.
///
/// `async` on purpose: `fs.*().await` runs BETWEEN Monty resumes (Monty's
/// `start`/`resume` are synchronous, we drive them from our async loop), so
/// there is no `block_on` and no runtime-in-runtime — the Phase-1 GATE-1b
/// property holds for Python too.
pub(crate) async fn dispatch_os_call(fs: &dyn Filesystem, call: &OsFunctionCall) -> Dispatched {
    match call {
        // ---- existence / type checks -------------------------------------
        OsFunctionCall::Exists(p) => Dispatched::ok(MontyObject::Bool(fs.exists(as_path(p)).await)),
        OsFunctionCall::IsFile(p) => {
            // A file is a path that exists but cannot be listed as a directory.
            let path = as_path(p);
            let is_file = fs.exists(path).await && fs.list(path).await.is_err();
            Dispatched::ok(MontyObject::Bool(is_file))
        }
        OsFunctionCall::IsDir(p) => Dispatched::ok(MontyObject::Bool(fs.list(as_path(p)).await.is_ok())),
        // The `Filesystem` seam does not expose symlinks — everything it hands
        // back is already a resolved regular file or directory.
        OsFunctionCall::IsSymlink(_) => Dispatched::ok(MontyObject::Bool(false)),

        // ---- reads --------------------------------------------------------
        OsFunctionCall::ReadText(p) => match fs.read(as_path(p), None).await {
            Ok(bytes) => Dispatched::ok(MontyObject::String(String::from_utf8_lossy(&bytes).into_owned())),
            Err(e) => Dispatched::Err(fs_error_to_exc(&e)),
        },
        OsFunctionCall::ReadBytes(p) => match fs.read(as_path(p), None).await {
            Ok(bytes) => Dispatched::ok(MontyObject::Bytes(bytes.to_vec())),
            Err(e) => Dispatched::Err(fs_error_to_exc(&e)),
        },
        OsFunctionCall::Stat(p) => match fs.stat(as_path(p)).await {
            Ok(meta) => {
                let mtime = meta.mtime_ms as f64 / 1000.0;
                // Directory detection: `list` succeeds only on a directory.
                let is_dir = fs.list(as_path(p)).await.is_ok();
                let obj = if is_dir {
                    monty::dir_stat(0o040_755, mtime)
                } else {
                    monty::file_stat(0o100_644, meta.size as i64, mtime)
                };
                Dispatched::ok(obj)
            }
            Err(e) => Dispatched::Err(fs_error_to_exc(&e)),
        },
        OsFunctionCall::Iterdir(p) => match fs.list(as_path(p)).await {
            Ok(entries) => {
                let base = Path::new(p.as_str());
                let items = entries
                    .into_iter()
                    .map(|e| {
                        let joined = base.join(&e.name);
                        MontyObject::String(joined.to_string_lossy().into_owned())
                    })
                    .collect();
                Dispatched::ok(MontyObject::List(items))
            }
            Err(e) => Dispatched::Err(fs_error_to_exc(&e)),
        },
        // We never expose host-absolute paths — echo the virtual path back so
        // `Path.resolve()` / `.absolute()` stay workspace-relative.
        OsFunctionCall::Resolve(p) | OsFunctionCall::Absolute(p) => {
            Dispatched::ok(MontyObject::String(p.as_str().to_string()))
        }

        // ---- writes (resume with the char/byte COUNT, never None) --------
        OsFunctionCall::WriteText(a) => {
            let count = a.data.chars().count() as i64;
            write_and_count(fs, a.path.as_str(), Bytes::from(a.data.clone().into_bytes()), false, count).await
        }
        OsFunctionCall::AppendText(a) => {
            let count = a.data.chars().count() as i64;
            write_and_count(fs, a.path.as_str(), Bytes::from(a.data.clone().into_bytes()), true, count).await
        }
        OsFunctionCall::WriteBytes(a) => {
            let count = a.data.len() as i64;
            write_and_count(fs, a.path.as_str(), Bytes::from(a.data.clone()), false, count).await
        }
        OsFunctionCall::AppendBytes(a) => {
            let count = a.data.len() as i64;
            write_and_count(fs, a.path.as_str(), Bytes::from(a.data.clone()), true, count).await
        }

        // ---- open (perform the open-time effect, return a handle) --------
        OsFunctionCall::Open(a) => open_effect(fs, a.path.as_str(), &a.mode).await,

        // ---- directory / path mutation -----------------------------------
        OsFunctionCall::Mkdir(a) => mkdir(fs, a.path.as_str(), a.parents, a.exist_ok).await,
        OsFunctionCall::Unlink(p) | OsFunctionCall::Rmdir(p) => match fs.remove(as_path(p)).await {
            Ok(()) => Dispatched::ok(MontyObject::None),
            Err(e) => Dispatched::Err(fs_error_to_exc(&e)),
        },
        OsFunctionCall::Rename(a) => match fs.rename(as_path(&a.src), as_path(&a.dst)).await {
            Ok(()) => Dispatched::ok(MontyObject::None),
            Err(e) => Dispatched::Err(fs_error_to_exc(&e)),
        },

        // ---- environment (curated allowlist — no secret ever leaks) ------
        OsFunctionCall::Getenv(a) => {
            if ENV_ALLOWLIST.contains(&a.key.as_str())
                && let Ok(v) = std::env::var(&a.key)
            {
                return Dispatched::ok(MontyObject::String(v));
            }
            // Unset (or non-allowlisted): hand back the caller's default,
            // which is `None` unless they passed `os.getenv(k, default)`.
            Dispatched::ok(a.default.clone())
        }
        OsFunctionCall::GetEnviron => {
            let pairs: Vec<(MontyObject, MontyObject)> = ENV_ALLOWLIST
                .iter()
                .filter_map(|k| std::env::var(k).ok().map(|v| {
                    (MontyObject::String((*k).to_string()), MontyObject::String(v))
                }))
                .collect();
            Dispatched::ok(MontyObject::Dict(pairs.into()))
        }

        // ---- clock (host system time, not a filesystem op) ---------------
        OsFunctionCall::DateToday => Dispatched::ok(today()),
        OsFunctionCall::DateTimeNow(_) => Dispatched::ok(now_naive()),

        // Placeholder left by `take_function_call`; never dispatched for real.
        OsFunctionCall::Used => Dispatched::ok(MontyObject::None),
    }
}

/// Perform a write (truncate or append) and, on success, resume with the
/// caller-computed `count` so Monty's `apply_write_position` stays happy.
async fn write_and_count(
    fs: &dyn Filesystem,
    path: &str,
    body: Bytes,
    append: bool,
    count: i64,
) -> Dispatched {
    let opts = WriteOpts {
        append,
        ..WriteOpts::default()
    };
    match fs.write(Path::new(path), body, opts).await {
        Ok(_) => Dispatched::ok(MontyObject::Int(count)),
        Err(e) => Dispatched::Err(fs_error_to_exc(&e)),
    }
}

/// `open(path, mode)` open-time effect. The host materializes the mode's
/// side-effect (existence check / truncate / create) and returns a
/// [`MontyObject::FileHandle`]; the actual byte IO surfaces later as
/// `ReadText` / `WriteText` calls handled above.
async fn open_effect(fs: &dyn Filesystem, path: &str, mode: &monty::FileMode) -> Dispatched {
    let p = Path::new(path);
    if !mode.create() {
        // `r` / `r+`: the file must already exist.
        if !fs.exists(p).await {
            return Dispatched::Err(MontyException::new(
                ExcType::FileNotFoundError,
                Some(format!("[Errno 2] No such file or directory: '{path}'")),
            ));
        }
    } else if mode.truncate() {
        // `w` / `w+`: truncate (create empty) on open.
        if let Err(e) = fs.write(p, Bytes::new(), WriteOpts::default()).await {
            return Dispatched::Err(fs_error_to_exc(&e));
        }
    } else {
        // `a` / `a+`: create if missing, preserving existing content.
        if !fs.exists(p).await
            && let Err(e) = fs.write(p, Bytes::new(), WriteOpts::default()).await
        {
            return Dispatched::Err(fs_error_to_exc(&e));
        }
    }
    Dispatched::ok(MontyObject::FileHandle(MontyFileHandle {
        path: path.to_string(),
        mode: *mode,
        position: 0,
    }))
}

/// `mkdir(path, parents, exist_ok)`. The `Filesystem` trait has no explicit
/// mkdir — a directory is implied by writing a file beneath it — so we write a
/// zero-byte marker (which materializes the parent dirs) then remove it,
/// leaving the directory behind. Mirrors the `emushell` builtin.
async fn mkdir(fs: &dyn Filesystem, path: &str, parents: bool, exist_ok: bool) -> Dispatched {
    let already = fs.list(Path::new(path)).await.is_ok();
    if already {
        if exist_ok {
            return Dispatched::ok(MontyObject::None);
        }
        return Dispatched::Err(MontyException::new(
            ExcType::FileExistsError,
            Some(format!("[Errno 17] File exists: '{path}'")),
        ));
    }
    let marker = format!("{}/.mkdir-keep", path.trim_end_matches('/'));
    let opts = WriteOpts {
        create_new: !parents,
        ..WriteOpts::default()
    };
    if let Err(e) = fs.write(Path::new(&marker), Bytes::new(), opts).await
        && !parents
    {
        return Dispatched::Err(fs_error_to_exc(&e));
    }
    let _ = fs.remove(Path::new(&marker)).await;
    Dispatched::ok(MontyObject::None)
}

fn as_path(p: &monty::MontyPath) -> &Path {
    Path::new(p.as_str())
}

/// Map a workspace `FsError` onto the Python exception CPython would raise for
/// the same condition, so the LLM sees `FileNotFoundError` / `PermissionError`
/// instead of an opaque host message.
fn fs_error_to_exc(e: &FsError) -> MontyException {
    let (ty, msg) = match e {
        FsError::NotFound(p) => (
            ExcType::FileNotFoundError,
            format!("[Errno 2] No such file or directory: '{p}'"),
        ),
        FsError::OutsideWorkspace(p) => (
            ExcType::PermissionError,
            format!("[Errno 13] Permission denied: '{p}'"),
        ),
        FsError::Binary(p) => (ExcType::OSError, format!("binary file: '{p}'")),
        FsError::TooLarge { path, .. } => (ExcType::OSError, format!("file too large: '{path}'")),
        FsError::InvalidInput(m) | FsError::Io(m) => (ExcType::OSError, m.clone()),
        FsError::Unsupported(m) => (ExcType::OSError, m.clone()),
    };
    MontyException::new(ty, Some(msg))
}

/// Host-clock `date.today()`.
fn today() -> MontyObject {
    use chrono::Datelike;
    let now = chrono::Local::now().naive_local().date();
    MontyObject::Date(MontyDate {
        year: now.year(),
        month: now.month() as u8,
        day: now.day() as u8,
    })
}

/// Host-clock naive `datetime.now()` (no timezone — matches `datetime.now()`
/// with no `tz` argument).
fn now_naive() -> MontyObject {
    use chrono::{Datelike, Timelike};
    let now = chrono::Local::now().naive_local();
    MontyObject::DateTime(MontyDateTime {
        year: now.year(),
        month: now.month() as u8,
        day: now.day() as u8,
        hour: now.hour() as u8,
        minute: now.minute() as u8,
        second: now.second() as u8,
        microsecond: now.nanosecond() / 1000,
        offset_seconds: None,
        timezone_name: None,
    })
}
