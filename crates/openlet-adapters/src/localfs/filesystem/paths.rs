//! Workspace-relative path resolution + boundary check.
//!
//! Lifted verbatim from the previous core helper at
//! `openlet-core/src/tools/builtins/paths.rs` — the logic is local-fs
//! specific, so it lives with the local impl now.

use std::path::{Component, Path, PathBuf};

use openlet_core::error::FsError;

fn lexical_normalize(path: &Path) -> Result<PathBuf, FsError> {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push("/"),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return Err(FsError::OutsideWorkspace(path.display().to_string()));
                }
            }
            Component::Normal(seg) => out.push(seg),
        }
    }
    Ok(out)
}

/// Resolve `input` relative to `root`, then verify the result stays
/// inside the workspace via canonicalization. Per amendment §L,
/// canonicalization walks to the deepest existing ancestor so we can
/// safely resolve write/edit targets that don't exist yet.
pub(crate) async fn resolve_in_workspace(root: &Path, input: &Path) -> Result<PathBuf, FsError> {
    let combined = if input.is_absolute() {
        input.to_path_buf()
    } else {
        root.join(input)
    };
    let lexical = lexical_normalize(&combined)?;
    let canonical = canonicalize_deepest_existing(&lexical).await?;
    let root_canonical = tokio::fs::canonicalize(root)
        .await
        .map_err(|e| FsError::Io(e.to_string()))?;
    if !canonical.starts_with(&root_canonical) {
        return Err(FsError::OutsideWorkspace(input.display().to_string()));
    }
    Ok(canonical)
}

async fn canonicalize_deepest_existing(path: &Path) -> Result<PathBuf, FsError> {
    if let Ok(c) = tokio::fs::canonicalize(path).await {
        return Ok(c);
    }
    let mut cursor = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    while let Some(parent) = cursor.parent() {
        if let Some(name) = cursor.file_name() {
            tail.push(name.to_os_string());
        } else {
            break;
        }
        cursor = parent.to_path_buf();
        if let Ok(c) = tokio::fs::canonicalize(&cursor).await {
            let mut out = c;
            for seg in tail.into_iter().rev() {
                out.push(seg);
            }
            return Ok(out);
        }
    }
    Err(FsError::Io(format!(
        "could not canonicalize any ancestor of {}",
        path.display()
    )))
}
