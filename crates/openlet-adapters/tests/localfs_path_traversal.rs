//! `LocalFilesystem` path traversal coverage.
//!
//! Cases:
//! - `read("../../etc/passwd")` → `FsError::OutsideWorkspace`
//! - Symlink escape (unix only) → resolver follows then OutsideWorkspace
//! - NUL byte in path → typed error (not panic)
//! - UTF-8 RTL override `\u{202E}` is NOT interpreted as traversal —
//!   it round-trips as a literal filename through `list`

mod common;

use std::path::Path;

use common::tempdir_workspace::WorkspaceFixture;
use openlet_adapters::localfs::LocalFilesystem;
use openlet_core::adapters::filesystem::Filesystem;
use openlet_core::error::FsError;

#[tokio::test]
async fn relative_dotdot_escape_returns_outside_workspace() {
    let fx = WorkspaceFixture::with_files(vec![("inner.txt", "hi")]);
    let fs = LocalFilesystem::new(fx.root().to_path_buf());
    let err = fs
        .read(Path::new("../../etc/passwd"), None)
        .await
        .expect_err("traversal must error");
    assert!(matches!(err, FsError::OutsideWorkspace(_)), "got {err:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_pointing_outside_workspace_is_rejected() {
    let fx = WorkspaceFixture::empty();
    // Place an external file outside the workspace root, in the
    // tempdir's sibling directory.
    let outside = fx.tempdir().join("external_secret.txt");
    std::fs::write(&outside, b"secret").unwrap();

    // Create a symlink inside the workspace pointing to the outside
    // file. Resolver canonicalizes through the link → escapes root.
    let link = fx.root().join("escape.txt");
    std::os::unix::fs::symlink(&outside, &link).unwrap();

    let fs = LocalFilesystem::new(fx.root().to_path_buf());
    let err = fs
        .read(Path::new("escape.txt"), None)
        .await
        .expect_err("symlink escape must error");
    assert!(matches!(err, FsError::OutsideWorkspace(_)), "got {err:?}");
}

#[tokio::test]
async fn rtl_override_codepoint_in_filename_is_not_traversal() {
    // U+202E (RIGHT-TO-LEFT OVERRIDE) is a unicode display-direction
    // codepoint that some sandboxes mistake for traversal. The
    // resolver must treat it as a literal char in a filename.
    let evil = "harmless\u{202E}.txt";
    let fx = WorkspaceFixture::with_files(vec![(evil, "ok")]);
    let fs = LocalFilesystem::new(fx.root().to_path_buf());

    // Read round-trips as a normal file.
    let bytes = fs.read(Path::new(evil), None).await.expect("read evil");
    assert_eq!(&bytes[..], b"ok");

    // List enumerates it.
    let entries = fs.list(Path::new(".")).await.expect("list");
    assert!(
        entries.iter().any(|e| e.name == evil),
        "RTL filename must round-trip through list"
    );
}

#[tokio::test]
async fn nul_byte_in_path_does_not_panic() {
    let fx = WorkspaceFixture::empty();
    let fs = LocalFilesystem::new(fx.root().to_path_buf());
    // `Path::new` accepts NUL but the OS rejects it. Confirm we get a
    // typed error, not a panic.
    let result = fs.read(Path::new("foo\0bar.txt"), None).await;
    let err = result.expect_err("NUL byte path must error");
    // Either Io (from the OS rejection) or InvalidInput is acceptable;
    // panic is not.
    assert!(
        matches!(
            err,
            FsError::Io(_) | FsError::InvalidInput(_) | FsError::NotFound(_)
        ),
        "got {err:?}"
    );
}
