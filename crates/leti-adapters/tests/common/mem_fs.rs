//! `MemFilesystem` — an in-process, in-memory `Filesystem` used ONLY to prove
//! executor/FS-impl independence in the parity suite.
//!
//! The whole thesis of this plan is that the emulated bash/python interpreters
//! hold nothing but `Arc<dyn Filesystem>`, so swapping the FS impl (local disk
//! vs cloud gRPC) leaves their behavior byte-identical. A real cloud backend
//! can't run in `cargo test` (that's Phase 6's gated live e2e), so this mock
//! stands in as a SECOND, structurally-unrelated impl: it stores bytes in a
//! `HashMap` with object-store semantics (implicit dirs), NOT `tokio::fs`.
//!
//! Fidelity-by-construction: glob and grep reuse the exact same `globset` and
//! `regex` crates `LocalFilesystem` uses, so the pattern dialect is identical
//! rather than approximated. If the interpreter produces the same stdout+exit
//! against both impls, the FS seam — not incidental disk behavior — is what the
//! interpreter depends on.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use bytes::Bytes;
use globset::Glob;
use leti_core::adapters::filesystem::{
    ByteRange, DirEntry, FileMeta, Filesystem, GlobOpts, GlobSort, GrepArgs, GrepHit, WriteOpts,
};
use leti_core::error::FsError;
use regex::RegexBuilder;

/// In-memory workspace: relative-path -> file bytes. Directories are implicit
/// (any prefix of an existing file key), mirroring object-store backends.
#[derive(Default)]
pub struct MemFilesystem {
    files: Mutex<HashMap<PathBuf, Vec<u8>>>,
}

impl MemFilesystem {
    #[must_use]
    pub fn new() -> Self {
        Self {
            files: Mutex::new(HashMap::new()),
        }
    }

    /// Seed the same relative-path files a `WorkspaceFixture` would, so a
    /// parity test can hand identical inputs to both impls.
    pub fn seed<'a, I>(files: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        let fs = Self::new();
        {
            let mut map = fs.files.lock().unwrap();
            for (path, body) in files {
                let rel = normalize_rel(Path::new(path)).expect("seed path must be in-workspace");
                map.insert(rel, body.as_bytes().to_vec());
            }
        }
        fs
    }
}

/// Normalize a workspace-relative path, rejecting any `..` escape or absolute
/// path exactly as `LocalFilesystem`'s boundary check does — so both impls
/// return `OutsideWorkspace` for the same inputs.
fn normalize_rel(path: &Path) -> Result<PathBuf, FsError> {
    if path.is_absolute() {
        return Err(FsError::OutsideWorkspace(path.display().to_string()));
    }
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::Normal(seg) => out.push(seg),
            Component::ParentDir => {
                if !out.pop() {
                    return Err(FsError::OutsideWorkspace(path.display().to_string()));
                }
            }
            // A rooted / prefixed component on a supposedly-relative path is
            // an escape attempt.
            Component::RootDir | Component::Prefix(_) => {
                return Err(FsError::OutsideWorkspace(path.display().to_string()));
            }
        }
    }
    Ok(out)
}

/// `true` if `dir` is a prefix directory of some existing file key.
fn is_implicit_dir(map: &HashMap<PathBuf, Vec<u8>>, dir: &Path) -> bool {
    if dir.as_os_str().is_empty() {
        return true; // workspace root
    }
    map.keys().any(|k| k.starts_with(dir) && k != dir)
}

#[async_trait]
impl Filesystem for MemFilesystem {
    async fn read(&self, path: &Path, range: Option<ByteRange>) -> Result<Bytes, FsError> {
        let rel = normalize_rel(path)?;
        let map = self.files.lock().unwrap();
        let body = map
            .get(&rel)
            .ok_or_else(|| FsError::NotFound(path.display().to_string()))?;
        match range {
            None => Ok(Bytes::from(body.clone())),
            Some(r) => {
                let start = (r.start as usize).min(body.len());
                let end = if r.len == 0 {
                    body.len()
                } else {
                    (start + r.len as usize).min(body.len())
                };
                Ok(Bytes::from(body[start..end].to_vec()))
            }
        }
    }

    async fn stat(&self, path: &Path) -> Result<FileMeta, FsError> {
        let rel = normalize_rel(path)?;
        let map = self.files.lock().unwrap();
        if let Some(body) = map.get(&rel) {
            Ok(FileMeta {
                size: body.len() as u64,
                mtime_ms: 0,
                is_binary: body.contains(&0),
                sha256: None,
            })
        } else if is_implicit_dir(&map, &rel) {
            Ok(FileMeta {
                size: 0,
                mtime_ms: 0,
                is_binary: false,
                sha256: None,
            })
        } else {
            Err(FsError::NotFound(path.display().to_string()))
        }
    }

    async fn exists(&self, path: &Path) -> bool {
        let Ok(rel) = normalize_rel(path) else {
            return false;
        };
        let map = self.files.lock().unwrap();
        map.contains_key(&rel) || is_implicit_dir(&map, &rel)
    }

    async fn write(&self, path: &Path, body: Bytes, opts: WriteOpts) -> Result<FileMeta, FsError> {
        let rel = normalize_rel(path)?;
        let mut map = self.files.lock().unwrap();
        if opts.create_new && map.contains_key(&rel) {
            return Err(FsError::InvalidInput(format!(
                "{}: file exists",
                path.display()
            )));
        }
        let bytes = if opts.append {
            let mut existing = map.get(&rel).cloned().unwrap_or_default();
            existing.extend_from_slice(&body);
            existing
        } else {
            body.to_vec()
        };
        let size = bytes.len() as u64;
        let is_binary = bytes.contains(&0);
        map.insert(rel, bytes);
        Ok(FileMeta {
            size,
            mtime_ms: 0,
            is_binary,
            sha256: None,
        })
    }

    async fn list(&self, path: &Path) -> Result<Vec<DirEntry>, FsError> {
        let rel = normalize_rel(path)?;
        let map = self.files.lock().unwrap();
        if !rel.as_os_str().is_empty() && !map.contains_key(&rel) && !is_implicit_dir(&map, &rel) {
            return Err(FsError::NotFound(path.display().to_string()));
        }
        // Collect the immediate children (files + subdirs) of `rel`.
        let mut files: Vec<(String, u64)> = Vec::new();
        let mut dirs: Vec<String> = Vec::new();
        for (key, body) in map.iter() {
            let Ok(tail) = key.strip_prefix(&rel) else {
                continue;
            };
            let mut comps = tail.components();
            let Some(first) = comps.next() else {
                continue;
            };
            let name = first.as_os_str().to_string_lossy().into_owned();
            if comps.next().is_some() {
                if !dirs.contains(&name) {
                    dirs.push(name);
                }
            } else {
                files.push((name, body.len() as u64));
            }
        }
        let mut out: Vec<DirEntry> = Vec::new();
        for name in dirs {
            out.push(DirEntry {
                name,
                is_dir: true,
                size: None,
            });
        }
        for (name, size) in files {
            out.push(DirEntry {
                name,
                is_dir: false,
                size: Some(size),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn glob(&self, pattern: &str, opts: GlobOpts) -> Result<Vec<PathBuf>, FsError> {
        // Same crate + matcher construction as `LocalFilesystem::glob`, so the
        // pattern dialect is identical by construction, not by imitation.
        let matcher = Glob::new(pattern)
            .map_err(|e| FsError::InvalidInput(e.to_string()))?
            .compile_matcher();
        let map = self.files.lock().unwrap();
        let mut hits: Vec<PathBuf> = map
            .keys()
            .filter(|rel| matcher.is_match(rel))
            .cloned()
            .collect();
        match opts.sort {
            // In-memory has no mtime; fall back to PathAsc. The interpreter's
            // word-expansion + `find` always request PathAsc, so this matches
            // the paths the parity scripts actually exercise.
            GlobSort::PathAsc | GlobSort::MtimeDesc => hits.sort(),
        }
        hits.truncate(opts.max_results);
        Ok(hits)
    }

    async fn grep(&self, args: GrepArgs) -> Result<Vec<GrepHit>, FsError> {
        // KNOWN DIVERGENCE (documented, matching the glob-sort note above):
        // `LocalFilesystem::grep` walks with `ignore::WalkBuilder` at its default
        // `git_ignore = true`, so it SKIPS `.gitignore`d files; this in-memory
        // impl has no gitignore notion and greps every key. Parity scripts must
        // therefore avoid seeding a `.gitignore` + an ignored-but-matching file,
        // or the two impls would disagree. The current parity suite seeds none,
        // so this stays latent — flagged here so a future script author doesn't
        // trip it. Same applies to the `GREP_MAX_*` resource caps `walk.rs`
        // enforces (immaterial at fixture scale, absent here).
        //
        // Same `regex` crate + case-insensitive builder as
        // `LocalFilesystem::grep`, so RE2 dialect + match semantics match.
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
        let map = self.files.lock().unwrap();
        // Sort keys so multi-file grep output is deterministic across runs
        // (the disk walk order is unspecified; parity scripts that compare
        // multi-file grep must therefore pipe through `sort`, but sorting the
        // walk here keeps single-file + globbed results stable).
        let mut keys: Vec<&PathBuf> = map.keys().collect();
        keys.sort();
        let mut hits: Vec<GrepHit> = Vec::new();
        for rel in keys {
            if hits.len() >= args.max_hits {
                break;
            }
            if let Some(g) = &path_glob
                && !g.is_match(rel)
            {
                continue;
            }
            let Ok(content) = std::str::from_utf8(&map[rel]) else {
                continue;
            };
            for (idx, line) in content.lines().enumerate() {
                if hits.len() >= args.max_hits {
                    break;
                }
                if re.is_match(line) {
                    let text = if line.len() > args.max_line_chars {
                        let mut cut = args.max_line_chars;
                        while !line.is_char_boundary(cut) {
                            cut -= 1;
                        }
                        format!("{}...", &line[..cut])
                    } else {
                        line.to_string()
                    };
                    hits.push(GrepHit {
                        path: rel.clone(),
                        line: (idx + 1) as u64,
                        text,
                    });
                }
            }
        }
        Ok(hits)
    }

    async fn remove(&self, path: &Path) -> Result<(), FsError> {
        let rel = normalize_rel(path)?;
        let mut map = self.files.lock().unwrap();
        if map.remove(&rel).is_some() {
            Ok(())
        } else if is_implicit_dir(&map, &rel) {
            // Non-empty implicit dir: the trait forbids recursive removal here
            // (callers walk leaf-first), so mirror that by refusing.
            Err(FsError::InvalidInput(format!(
                "{}: directory not empty",
                path.display()
            )))
        } else {
            Err(FsError::NotFound(path.display().to_string()))
        }
    }

    async fn rename(&self, from: &Path, to: &Path) -> Result<(), FsError> {
        let from_rel = normalize_rel(from)?;
        let to_rel = normalize_rel(to)?;
        let mut map = self.files.lock().unwrap();
        let body = map
            .remove(&from_rel)
            .ok_or_else(|| FsError::NotFound(from.display().to_string()))?;
        map.insert(to_rel, body);
        Ok(())
    }
}
