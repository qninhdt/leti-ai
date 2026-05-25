//! Read / write / stat / list ops on a workspace-rooted local FS.

use std::path::Path;
use std::time::UNIX_EPOCH;

use bytes::Bytes;
use openlet_core::adapters::filesystem::{ByteRange, DirEntry, FileMeta, WriteOpts};
use openlet_core::error::FsError;
use tempfile::NamedTempFile;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom};

use super::paths::resolve_in_workspace;

const SAMPLE_BYTES: usize = 4096;
/// Hard cap on a single `read` allocation. The tool layer should pass a
/// smaller cap, but this is the floor that bounds memory under TOCTOU
/// (file growing between stat and read).
const MAX_READ_BYTES: u64 = 8 * 1024 * 1024;

pub(crate) async fn read(
    root: &Path,
    path: &Path,
    range: Option<ByteRange>,
) -> Result<Bytes, FsError> {
    let resolved = resolve_in_workspace(root, path).await?;
    let meta = fs::metadata(&resolved)
        .await
        .map_err(|e| map_io_not_found(&resolved, e))?;
    let total = meta.len();

    let (start, len) = match range {
        Some(r) => {
            if r.start > total {
                return Err(FsError::InvalidInput(format!(
                    "range start {} > file size {}",
                    r.start, total
                )));
            }
            let start = r.start;
            let len = if r.len == 0 {
                total - start
            } else {
                r.len.min(total - start)
            };
            (start, len)
        }
        None => (0, total),
    };
    // Bound allocation by MAX_READ_BYTES so a file that grows between
    // stat and read (TOCTOU) cannot blow memory. This is a floor; tool
    // layer caps tighter (1 MiB for `read` tool).
    let len = len.min(MAX_READ_BYTES);

    let mut file = fs::File::open(&resolved)
        .await
        .map_err(|e| FsError::Io(e.to_string()))?;
    if start > 0 {
        file.seek(SeekFrom::Start(start))
            .await
            .map_err(|e| FsError::Io(e.to_string()))?;
    }
    let mut buf = Vec::with_capacity(usize::try_from(len).unwrap_or(0));
    let mut take = file.take(len);
    take.read_to_end(&mut buf)
        .await
        .map_err(|e| FsError::Io(e.to_string()))?;
    Ok(Bytes::from(buf))
}

pub(crate) async fn stat(root: &Path, path: &Path) -> Result<FileMeta, FsError> {
    let resolved = resolve_in_workspace(root, path).await?;
    let meta = fs::metadata(&resolved)
        .await
        .map_err(|e| map_io_not_found(&resolved, e))?;
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);

    let is_binary = if meta.is_file() {
        sniff_binary(&resolved).await.unwrap_or(false)
    } else {
        false
    };

    Ok(FileMeta {
        size: meta.len(),
        mtime_ms,
        is_binary,
        sha256: None,
    })
}

pub(crate) async fn exists(root: &Path, path: &Path) -> bool {
    let Ok(resolved) = resolve_in_workspace(root, path).await else {
        return false;
    };
    fs::metadata(&resolved).await.is_ok()
}

pub(crate) async fn write(
    root: &Path,
    path: &Path,
    body: Bytes,
    opts: WriteOpts,
) -> Result<FileMeta, FsError> {
    let resolved = resolve_in_workspace(root, path).await?;

    if let Some(parent) = resolved.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| FsError::Io(e.to_string()))?;
        // Re-canonicalize parent post-create. resolve_in_workspace resolves
        // the deepest *pre-existing* ancestor and lexically appends the tail,
        // so a symlink swap in the not-yet-created portion would slip past
        // the boundary check. Recheck against root_canonical now that every
        // dir on the path actually exists.
        let parent_canonical = fs::canonicalize(parent)
            .await
            .map_err(|e| FsError::Io(e.to_string()))?;
        let root_canonical = fs::canonicalize(root)
            .await
            .map_err(|e| FsError::Io(e.to_string()))?;
        if !parent_canonical.starts_with(&root_canonical) {
            return Err(FsError::OutsideWorkspace(path.display().to_string()));
        }
    }

    if opts.atomic {
        if opts.create_new {
            // Atomic + create_new: stage to tempfile + rename, but reject
            // pre-existing target via OpenOptions::create_new on the
            // tempfile so two concurrent calls can't both pass a stat.
            // Closes VULN-F7 (create_new TOCTOU race).
            if fs::metadata(&resolved).await.is_ok() {
                return Err(FsError::InvalidInput(format!(
                    "file already exists: {}",
                    resolved.display()
                )));
            }
        }
        write_atomic(&resolved, &body).await?;
    } else if opts.create_new {
        // Non-atomic + create_new: use OpenOptions::create_new(true) so
        // the kernel atomically rejects pre-existing targets — no TOCTOU
        // window between metadata-check and open. Closes VULN-F7.
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&resolved)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    FsError::InvalidInput(format!("file already exists: {}", resolved.display()))
                } else {
                    FsError::Io(e.to_string())
                }
            })?;
        file.write_all(&body)
            .await
            .map_err(|e| FsError::Io(e.to_string()))?;
        file.flush()
            .await
            .map_err(|e| FsError::Io(e.to_string()))?;
    } else {
        fs::write(&resolved, &body)
            .await
            .map_err(|e| FsError::Io(e.to_string()))?;
    }

    let meta = fs::metadata(&resolved)
        .await
        .map_err(|e| FsError::Io(e.to_string()))?;
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    Ok(FileMeta {
        size: meta.len(),
        mtime_ms,
        is_binary: false,
        sha256: None,
    })
}

async fn write_atomic(target: &Path, body: &[u8]) -> Result<(), FsError> {
    let parent = target
        .parent()
        .ok_or_else(|| FsError::Io("write target has no parent dir".into()))?
        .to_path_buf();
    let target_clone = target.to_path_buf();
    let body_clone = body.to_vec();

    // tempfile is sync; offload to blocking pool.
    tokio::task::spawn_blocking(move || -> Result<(), FsError> {
        let tmp = NamedTempFile::new_in(&parent).map_err(|e| FsError::Io(e.to_string()))?;
        std::fs::write(tmp.path(), &body_clone).map_err(|e| FsError::Io(e.to_string()))?;
        tmp.persist(&target_clone)
            .map_err(|e| FsError::Io(e.error.to_string()))?;
        Ok(())
    })
    .await
    .map_err(|e| FsError::Io(format!("atomic write join: {e}")))??;
    Ok(())
}

pub(crate) async fn list(root: &Path, path: &Path) -> Result<Vec<DirEntry>, FsError> {
    let resolved = resolve_in_workspace(root, path).await?;
    let mut rd = fs::read_dir(&resolved)
        .await
        .map_err(|e| map_io_not_found(&resolved, e))?;
    let mut out = Vec::new();
    while let Some(entry) = rd
        .next_entry()
        .await
        .map_err(|e| FsError::Io(e.to_string()))?
    {
        let ft = entry
            .file_type()
            .await
            .map_err(|e| FsError::Io(e.to_string()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let size = if ft.is_file() {
            entry.metadata().await.ok().map(|m| m.len())
        } else {
            None
        };
        out.push(DirEntry {
            name,
            is_dir: ft.is_dir(),
            size,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

async fn sniff_binary(path: &Path) -> std::io::Result<bool> {
    let mut f = fs::File::open(path).await?;
    let mut buf = vec![0u8; SAMPLE_BYTES];
    let n = f.read(&mut buf).await?;
    let head = &buf[..n];
    if head.contains(&0u8) {
        return Ok(true);
    }
    let non_printable = head
        .iter()
        .filter(|b| !(matches!(**b, b'\n' | b'\r' | b'\t') || (0x20..=0x7e).contains(*b)))
        .count();
    Ok(!head.is_empty() && (non_printable * 100 / head.len().max(1)) > 30)
}

fn map_io_not_found(path: &Path, e: std::io::Error) -> FsError {
    if e.kind() == std::io::ErrorKind::NotFound {
        FsError::NotFound(path.display().to_string())
    } else {
        FsError::Io(e.to_string())
    }
}
