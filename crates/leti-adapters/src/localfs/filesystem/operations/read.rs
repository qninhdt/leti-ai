//! Read-side ops: `read`, `stat`, `exists`, `list`, plus `sniff_binary`
//! used by `stat`.

use std::path::Path;

use bytes::Bytes;
use leti_core::adapters::filesystem::{ByteRange, DirEntry, FileMeta};
use leti_core::error::FsError;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, SeekFrom};

use super::super::paths::resolve_in_workspace;
use super::meta::mtime_ms;

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
    let mtime_ms = mtime_ms(&meta);

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
