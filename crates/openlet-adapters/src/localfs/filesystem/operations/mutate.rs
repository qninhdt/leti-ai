//! Mutating ops: `remove` (file or empty/recursive dir) + `rename`.
//!
//! Both resolve every path through `resolve_in_workspace` so a `..` or
//! symlink escape is rejected with `OutsideWorkspace` before any syscall
//! touches the target — the same boundary guarantee `read`/`write` give.

use std::path::Path;

use openlet_core::error::FsError;
use tokio::fs;

use super::super::paths::resolve_in_workspace;

pub(crate) async fn remove(root: &Path, path: &Path) -> Result<(), FsError> {
    let resolved = resolve_in_workspace(root, path).await?;
    let meta = fs::symlink_metadata(&resolved)
        .await
        .map_err(|e| map_not_found(&resolved, e))?;
    if meta.is_dir() {
        // Non-recursive by contract: remove an empty directory only. A `rm -r`
        // builtin walks the tree and removes leaf-first through this same
        // method, so the workspace-boundary check runs on every path rather
        // than being bypassed by a single host-side recursive delete. This
        // also prevents a symlinked subdirectory from being followed off-root.
        fs::remove_dir(&resolved)
            .await
            .map_err(|e| FsError::Io(e.to_string()))?;
    } else {
        fs::remove_file(&resolved)
            .await
            .map_err(|e| FsError::Io(e.to_string()))?;
    }
    Ok(())
}

pub(crate) async fn rename(root: &Path, from: &Path, to: &Path) -> Result<(), FsError> {
    let src = resolve_in_workspace(root, from).await?;
    // Resolve the destination too so a rename cannot move a file outside
    // the workspace via `mv f ../escape`. `resolve_in_workspace` walks to
    // the deepest existing ancestor, so a not-yet-existing dst still
    // boundary-checks correctly.
    let dst = resolve_in_workspace(root, to).await?;

    // Ensure the destination parent exists so `mv f subdir/f` works even
    // when `subdir` is fresh — matches `write`'s create-parents behavior.
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| FsError::Io(e.to_string()))?;
    }

    fs::rename(&src, &dst)
        .await
        .map_err(|e| map_not_found(&src, e))?;
    Ok(())
}

fn map_not_found(path: &Path, e: std::io::Error) -> FsError {
    if e.kind() == std::io::ErrorKind::NotFound {
        FsError::NotFound(path.display().to_string())
    } else {
        FsError::Io(e.to_string())
    }
}
