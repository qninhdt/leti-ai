//! Glob + grep over a workspace-rooted local FS via `ignore::WalkBuilder`.
//!
//! `respect_gitignore = false` would skip the `.gitignore` filter; the
//! `ignore` crate honors it by default. Both ops run on the blocking
//! pool because the underlying iterator is sync.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use globset::{Glob, GlobMatcher};
use ignore::WalkBuilder;
use openlet_core::adapters::filesystem::{GlobOpts, GlobSort, GrepArgs, GrepHit};
use openlet_core::error::FsError;
use regex::RegexBuilder;

/// Cap grep file size to bound memory + tail-latency on accidental
/// loopback symlinks (e.g. `/proc/self/mem`) and giant binaries.
const GREP_MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Cap total bytes read across the walk. Without this, a workspace of
/// 50k 8 MiB files = 400 GiB read per grep call. Saturates the blocking
/// pool. Aborts gracefully when exceeded.
const GREP_MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
/// Cap files visited so a workspace of millions of small files cannot
/// stall a single grep dispatch indefinitely.
const GREP_MAX_FILES: usize = 50_000;

/// Floor `index` to the nearest UTF-8 char boundary at or below it.
fn floor_char_boundary(s: &str, mut index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(index) {
        index -= 1;
    }
    index
}

pub(crate) async fn glob(
    root: &Path,
    pattern: &str,
    opts: GlobOpts,
) -> Result<Vec<PathBuf>, FsError> {
    let matcher: GlobMatcher = Glob::new(pattern)
        .map_err(|e| FsError::InvalidInput(e.to_string()))?
        .compile_matcher();
    let root = root.to_path_buf();

    tokio::task::spawn_blocking(move || -> Vec<PathBuf> {
        let mut hits: Vec<(PathBuf, SystemTime)> = Vec::new();
        let walker = WalkBuilder::new(&root)
            .hidden(false)
            .git_ignore(opts.respect_gitignore)
            .git_global(opts.respect_gitignore)
            .git_exclude(opts.respect_gitignore)
            .require_git(false)
            .build();
        for entry in walker.flatten() {
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let rel = entry.path().strip_prefix(&root).unwrap_or(entry.path());
            if !matcher.is_match(rel) {
                continue;
            }
            let mtime = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            // Return workspace-relative paths per the `Filesystem` trait
            // contract ("All paths are workspace-relative"). Leaking the
            // absolute host path would both break that contract and expose
            // the tempdir/cloud-mount prefix to the caller.
            hits.push((rel.to_path_buf(), mtime));
        }
        match opts.sort {
            GlobSort::PathAsc => hits.sort_by(|a, b| a.0.cmp(&b.0)),
            GlobSort::MtimeDesc => hits.sort_by_key(|b| std::cmp::Reverse(b.1)),
        }
        hits.truncate(opts.max_results);
        hits.into_iter().map(|(p, _)| p).collect()
    })
    .await
    .map_err(|e| FsError::Io(format!("glob join: {e}")))
}

pub(crate) async fn grep(root: &Path, args: GrepArgs) -> Result<Vec<GrepHit>, FsError> {
    let re = RegexBuilder::new(&args.pattern)
        .case_insensitive(args.case_insensitive)
        .build()
        .map_err(|e| FsError::InvalidInput(e.to_string()))?;

    let path_glob = match args.path_glob.as_deref() {
        Some(p) => Some(
            Glob::new(p)
                .map_err(|e| FsError::InvalidInput(e.to_string()))?
                .compile_matcher(),
        ),
        None => None,
    };

    let root = root.to_path_buf();
    let max_hits = args.max_hits;
    let max_line_chars = args.max_line_chars;

    tokio::task::spawn_blocking(move || -> Vec<GrepHit> {
        let mut hits: Vec<GrepHit> = Vec::new();
        let mut files_visited: usize = 0;
        let mut total_bytes_read: u64 = 0;
        let walker = WalkBuilder::new(&root).hidden(false).build();
        'walk: for entry in walker.flatten() {
            if hits.len() >= max_hits {
                break 'walk;
            }
            if files_visited >= GREP_MAX_FILES {
                break 'walk;
            }
            if total_bytes_read >= GREP_MAX_TOTAL_BYTES {
                break 'walk;
            }
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            files_visited += 1;
            let path = entry.path();
            let rel = path.strip_prefix(&root).unwrap_or(path);
            if let Some(g) = &path_glob {
                if !g.is_match(rel) {
                    continue;
                }
            }
            let content = match std::fs::metadata(path) {
                Ok(m) if m.len() > GREP_MAX_FILE_BYTES => continue,
                Ok(m) => {
                    total_bytes_read = total_bytes_read.saturating_add(m.len());
                    match std::fs::read_to_string(path) {
                        Ok(s) => s,
                        Err(_) => continue,
                    }
                }
                Err(_) => continue,
            };
            for (idx, line) in content.lines().enumerate() {
                if hits.len() >= max_hits {
                    break 'walk;
                }
                if re.is_match(line) {
                    let text = if line.len() > max_line_chars {
                        let cut = floor_char_boundary(line, max_line_chars);
                        format!("{}...", &line[..cut])
                    } else {
                        line.to_string()
                    };
                    hits.push(GrepHit {
                        path: rel.to_path_buf(),
                        line: (idx + 1) as u64,
                        text,
                    });
                }
            }
        }
        hits
    })
    .await
    .map_err(|e| FsError::Io(format!("grep join: {e}")))
}
