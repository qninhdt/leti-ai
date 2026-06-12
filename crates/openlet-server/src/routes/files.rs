//! `/v1/files*` — workspace file listing + content for the TUI's @-mention
//! feature.
//!
//! Backed by the default agent's `Filesystem` adapter: listing is a
//! gitignore-aware recursive walk of the workspace, content is a jailed
//! read. Absolute paths and `..` traversal are rejected with 400, and
//! secret file patterns (`.env*`, `*.pem`, `id_rsa*`, `*.key`,
//! `credentials*`, `*.pfx`) are excluded from both listing and content so
//! a secret can never leak through the @-mention surface. The adapter
//! enforces the realpath workspace-root jail underneath; this layer adds
//! the request-shape validation + secret filter on top.

use axum::Json;
use axum::extract::{Query, State};
use openlet_core::adapters::filesystem::{ByteRange, GlobOpts, GlobSort};
use serde::{Deserialize, Serialize};
use std::path::Path;
use utoipa::{IntoParams, ToSchema};

use crate::app_state::AppState;
use crate::error::AppError;

/// Recursive glob cap. Bounds the workspace walk so a huge repo can't
/// stall the @-mention list; the TUI filters client-side over this set.
const MAX_FILES: usize = 500;

/// Content read cap (256 KiB). Larger files are clamped and flagged
/// `truncated` so the embed path never pulls an unbounded blob.
const MAX_CONTENT_BYTES: u64 = 256 * 1024;

/// File classification surfaced to the client (drives badge styling + whether
/// content is embeddable). `text` is embeddable; `image`/`pdf` are badge-only.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FileKindDto {
    Text,
    Image,
    Pdf,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FileEntryDto {
    pub path: String,
    #[serde(rename = "type")]
    pub kind: FileKindDto,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FileListDto {
    pub files: Vec<FileEntryDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FileContentDto {
    pub path: String,
    #[serde(rename = "type")]
    pub kind: FileKindDto,
    /// Embeddable text content. Absent for unsupported (binary) types.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// True when content was clamped to the server's size cap.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// True for types whose content cannot be embedded as text (image/pdf).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unsupported: bool,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct FileListQuery {
    /// Case-insensitive substring filter over workspace-relative paths.
    #[serde(default)]
    pub query: String,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct FileContentQuery {
    /// Workspace-relative path of the file to read.
    pub path: String,
}

// Secret patterns excluded from listing + content, mirroring the TUI/plan
// guard. Matched against the path's final component, split on BOTH separators
// (a backslash path must not slip the filter) and case-folded (case-insensitive
// filesystems would otherwise open `.ENV`/`ID_RSA`).
fn is_secret_path(path: &str) -> bool {
    let name = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase();
    name.starts_with(".env")
        || name.ends_with(".pem")
        || name.starts_with("id_rsa")
        || name.ends_with(".key")
        || name.starts_with("credentials")
        || name.ends_with(".pfx")
}

// Reject absolute paths and `..` traversal so the contract is safe from day
// one. Returns a 400 AppError on violation.
fn validate_relative_path(path: &str) -> Result<(), AppError> {
    if path.is_empty() {
        return Err(AppError::bad_request("invalid_path", "empty path"));
    }
    if path.starts_with('/') || path.starts_with('\\') {
        return Err(AppError::bad_request(
            "absolute_path_rejected",
            "absolute paths are not allowed",
        ));
    }
    // Windows drive-letter absolute (C:\...).
    if path.len() >= 2 && path.as_bytes()[1] == b':' {
        return Err(AppError::bad_request(
            "absolute_path_rejected",
            "absolute paths are not allowed",
        ));
    }
    if path.split(['/', '\\']).any(|seg| seg == "..") {
        return Err(AppError::bad_request(
            "path_traversal_rejected",
            "path traversal is not allowed",
        ));
    }
    Ok(())
}

/// Classify a path by extension. `text` is embeddable; `image`/`pdf` are
/// badge-only (content is never read as text for them). The extension is
/// taken from the final path component so a dotted directory name can't
/// misclassify an extensionless file.
fn classify(path: &str) -> FileKindDto {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" => FileKindDto::Image,
        "pdf" => FileKindDto::Pdf,
        _ => FileKindDto::Text,
    }
}

/// `GET /v1/files?query=` — list/search workspace files.
#[utoipa::path(
    get,
    path = "/v1/files",
    tag = "files",
    params(FileListQuery),
    responses((status = 200, description = "Matching workspace files", body = FileListDto))
)]
pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<FileListQuery>,
) -> Result<Json<FileListDto>, AppError> {
    let agent = state
        .agents
        .get(&state.default_agent_id)
        .ok_or_else(|| AppError::internal("agent_unavailable", "default agent not registered"))?;
    let fs = &agent.fs;

    let paths = fs
        .glob(
            "**/*",
            GlobOpts {
                respect_gitignore: true,
                max_results: MAX_FILES,
                sort: GlobSort::PathAsc,
            },
        )
        .await?;

    let needle = q.query.to_lowercase();
    let mut files = Vec::new();
    for abs in &paths {
        // The trait returns absolute paths; the TUI + `/content` contract
        // speak workspace-relative, so strip the root we threaded into state.
        let rel = abs
            .strip_prefix(&state.workspace_root)
            .unwrap_or(abs)
            .to_string_lossy()
            .replace('\\', "/");
        if is_secret_path(&rel) {
            continue;
        }
        if !needle.is_empty() && !rel.to_lowercase().contains(&needle) {
            continue;
        }
        let kind = classify(&rel);
        files.push(FileEntryDto { path: rel, kind });
    }
    Ok(Json(FileListDto { files }))
}

/// `GET /v1/files/content?path=` — read a workspace file's content.
#[utoipa::path(
    get,
    path = "/v1/files/content",
    tag = "files",
    params(FileContentQuery),
    responses(
        (status = 200, description = "File content (or unsupported flag)", body = FileContentDto),
        (status = 400, description = "Absolute path / traversal rejected"),
        (status = 404, description = "File not found"),
    )
)]
pub async fn content(
    State(state): State<AppState>,
    Query(q): Query<FileContentQuery>,
) -> Result<Json<FileContentDto>, AppError> {
    validate_relative_path(&q.path)?;
    if is_secret_path(&q.path) {
        // Treat as not-found rather than confirming the secret file exists.
        return Err(AppError::not_found("file_not_found", "file not found"));
    }

    let kind = classify(&q.path);
    if kind != FileKindDto::Text {
        // Binary types are badge-only — never read their bytes.
        return Ok(Json(FileContentDto {
            path: q.path,
            kind,
            content: None,
            truncated: false,
            unsupported: true,
        }));
    }

    let agent = state
        .agents
        .get(&state.default_agent_id)
        .ok_or_else(|| AppError::internal("agent_unavailable", "default agent not registered"))?;
    let fs = &agent.fs;

    // Read one byte past the cap so we can tell a clamped file from one that
    // exactly fills it. `len = 0` would mean "to end", so request cap+1.
    let range = ByteRange {
        start: 0,
        len: MAX_CONTENT_BYTES + 1,
    };
    let bytes = match fs.read(Path::new(&q.path), Some(range)).await {
        Ok(b) => b,
        Err(openlet_core::error::FsError::NotFound(_)) => {
            return Err(AppError::not_found("file_not_found", "file not found"));
        }
        Err(e) => return Err(e.into()),
    };

    let truncated = bytes.len() as u64 > MAX_CONTENT_BYTES;
    let slice = if truncated {
        &bytes[..MAX_CONTENT_BYTES as usize]
    } else {
        &bytes[..]
    };
    let text = String::from_utf8_lossy(slice).into_owned();

    Ok(Json(FileContentDto {
        path: q.path,
        kind,
        content: Some(text),
        truncated,
        unsupported: false,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_absolute_and_traversal() {
        assert!(validate_relative_path("/etc/passwd").is_err());
        assert!(validate_relative_path("C:\\secrets").is_err());
        assert!(validate_relative_path("../escape").is_err());
        assert!(validate_relative_path("src/../../etc").is_err());
        assert!(validate_relative_path("").is_err());
        assert!(validate_relative_path("src/app.tsx").is_ok());
    }

    #[test]
    fn excludes_secret_patterns() {
        assert!(is_secret_path(".env"));
        assert!(is_secret_path(".env.local"));
        assert!(is_secret_path("deploy/key.pem"));
        assert!(is_secret_path("home/id_rsa"));
        assert!(is_secret_path("certs/server.key"));
        assert!(is_secret_path("credentials.json"));
        assert!(!is_secret_path("src/app.tsx"));
        // Backslash-separated basename must still be matched.
        assert!(is_secret_path("secrets\\.env"));
        assert!(is_secret_path("certs\\server.key"));
        // Case-insensitive: real FS on macOS/Windows would open these.
        assert!(is_secret_path(".ENV"));
        assert!(is_secret_path("home/ID_RSA"));
        assert!(is_secret_path("CREDENTIALS.json"));
        assert!(is_secret_path("deploy/KEY.PEM"));
    }
}
