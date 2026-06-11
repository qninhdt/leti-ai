//! `/v1/files*` — workspace file listing + content for the TUI's @-mention
//! feature.
//!
//! **Mock data for now.** These endpoints return a fixed file list + synthetic
//! content so the TUI's autocomplete + embedding can be built and tested before
//! real filesystem wiring lands. The response *shape* is locked here (and in the
//! OpenAPI doc) so the real FS implementation is a drop-in behind the same
//! contract.
//!
//! Security (enforced even on mock data, so the contract is correct from day
//! one): absolute paths and `..` traversal are rejected with 400, and secret
//! file patterns (`.env*`, `*.pem`, `id_rsa*`, `*.key`, `credentials*`,
//! `*.pfx`) are excluded from both listing and content. When real FS wiring
//! lands it must additionally enforce a realpath workspace-root jail.

use axum::Json;
use axum::extract::Query;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::error::AppError;

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
    /// Case-insensitive substring filter over the mock file list.
    #[serde(default)]
    pub query: String,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct FileContentQuery {
    /// Workspace-relative path of the file to read.
    pub path: String,
}

// Mock workspace file list. Stands in for a real FS walk until that lands.
const MOCK_FILES: &[(&str, FileKindDto)] = &[
    ("src/app.tsx", FileKindDto::Text),
    ("src/store/index.ts", FileKindDto::Text),
    ("src/api/client.ts", FileKindDto::Text),
    ("README.md", FileKindDto::Text),
    ("Cargo.toml", FileKindDto::Text),
    ("docs/logo.png", FileKindDto::Image),
    ("docs/spec.pdf", FileKindDto::Pdf),
];

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

/// `GET /v1/files?query=` — list/search workspace files (mock).
#[utoipa::path(
    get,
    path = "/v1/files",
    tag = "files",
    params(FileListQuery),
    responses((status = 200, description = "Matching workspace files", body = FileListDto))
)]
pub async fn list(Query(q): Query<FileListQuery>) -> Json<FileListDto> {
    let needle = q.query.to_lowercase();
    let files = MOCK_FILES
        .iter()
        .filter(|(path, _)| !is_secret_path(path))
        .filter(|(path, _)| needle.is_empty() || path.to_lowercase().contains(&needle))
        .map(|(path, kind)| FileEntryDto {
            path: (*path).to_string(),
            kind: *kind,
        })
        .collect();
    Json(FileListDto { files })
}

/// `GET /v1/files/content?path=` — read a workspace file's content (mock).
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
pub async fn content(Query(q): Query<FileContentQuery>) -> Result<Json<FileContentDto>, AppError> {
    validate_relative_path(&q.path)?;
    if is_secret_path(&q.path) {
        // Treat as not-found rather than confirming the secret file exists.
        return Err(AppError::not_found("file_not_found", "file not found"));
    }
    let Some((path, kind)) = MOCK_FILES.iter().find(|(p, _)| *p == q.path) else {
        return Err(AppError::not_found("file_not_found", "file not found"));
    };
    if *kind != FileKindDto::Text {
        return Ok(Json(FileContentDto {
            path: (*path).to_string(),
            kind: *kind,
            content: None,
            truncated: false,
            unsupported: true,
        }));
    }
    Ok(Json(FileContentDto {
        path: (*path).to_string(),
        kind: *kind,
        content: Some(format!("// mock content for {path}\n")),
        truncated: false,
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
