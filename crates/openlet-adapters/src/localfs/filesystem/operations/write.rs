//! Write-side ops: `write` + `write_atomic` helper.

use std::path::Path;

use bytes::Bytes;
use openlet_core::adapters::filesystem::{FileMeta, WriteOpts};
use openlet_core::error::FsError;
use tempfile::NamedTempFile;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::super::paths::resolve_in_workspace;
use super::meta::mtime_ms;

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
        // Atomic + create_new: the previous metadata-stat-then-persist
        // pattern was a TOCTOU window (`tempfile.persist` always
        // clobbers via rename(2)). `persist_noclobber` uses linkat with
        // RENAME_NOREPLACE on Linux, so the kernel atomically rejects
        // pre-existing targets.
        write_atomic(&resolved, &body, opts.create_new).await?;
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
        file.flush().await.map_err(|e| FsError::Io(e.to_string()))?;
    } else {
        fs::write(&resolved, &body)
            .await
            .map_err(|e| FsError::Io(e.to_string()))?;
    }

    let meta = fs::metadata(&resolved)
        .await
        .map_err(|e| FsError::Io(e.to_string()))?;
    let mtime_ms = mtime_ms(&meta);
    Ok(FileMeta {
        size: meta.len(),
        mtime_ms,
        is_binary: false,
        sha256: None,
    })
}

async fn write_atomic(target: &Path, body: &[u8], no_clobber: bool) -> Result<(), FsError> {
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
        if no_clobber {
            tmp.persist_noclobber(&target_clone).map_err(|e| {
                if e.error.kind() == std::io::ErrorKind::AlreadyExists {
                    FsError::InvalidInput(format!(
                        "file already exists: {}",
                        target_clone.display()
                    ))
                } else {
                    FsError::Io(e.error.to_string())
                }
            })?;
        } else {
            tmp.persist(&target_clone)
                .map_err(|e| FsError::Io(e.error.to_string()))?;
        }
        Ok(())
    })
    .await
    .map_err(|e| FsError::Io(format!("atomic write join: {e}")))??;
    Ok(())
}
