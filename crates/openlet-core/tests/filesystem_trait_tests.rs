//! `Filesystem` trait contract tests against `LocalFilesystem`.
//!
//! Validates the seam openlet-core uses to call into a workspace
//! filesystem: boundary enforcement, byte-range read, atomic write,
//! gitignore-aware glob, regex grep, list ordering, and error mapping.
//! Cloud `Filesystem` impls must satisfy the same observable contract.

use std::path::PathBuf;

use openlet_adapters::localfs::LocalFilesystem;
use openlet_core::adapters::filesystem::{
    ByteRange, Filesystem, GlobOpts, GlobSort, GrepArgs, WriteOpts,
};
use openlet_core::error::FsError;
use tempfile::TempDir;

fn fs_at(dir: &TempDir) -> LocalFilesystem {
    LocalFilesystem::new(dir.path().to_path_buf())
}

#[tokio::test]
async fn write_then_read_round_trip() {
    let dir = TempDir::new().unwrap();
    let fs = fs_at(&dir);
    let path = PathBuf::from("hello.txt");
    let body = bytes::Bytes::from_static(b"hello world\n");

    let meta = fs
        .write(&path, body.clone(), WriteOpts::default())
        .await
        .unwrap();
    assert_eq!(meta.size, body.len() as u64);

    let read_back = fs.read(&path, None).await.unwrap();
    assert_eq!(read_back.as_ref(), body.as_ref());
}

#[tokio::test]
async fn read_with_byte_range_returns_slice() {
    let dir = TempDir::new().unwrap();
    let fs = fs_at(&dir);
    let path = PathBuf::from("range.txt");
    let body = bytes::Bytes::from_static(b"abcdefghij");
    fs.write(&path, body, WriteOpts::default()).await.unwrap();

    let slice = fs
        .read(&path, Some(ByteRange { start: 2, len: 3 }))
        .await
        .unwrap();
    assert_eq!(slice.as_ref(), b"cde");
}

#[tokio::test]
async fn stat_reports_size_and_binary_flag() {
    let dir = TempDir::new().unwrap();
    let fs = fs_at(&dir);
    let text_path = PathBuf::from("text.txt");
    let bin_path = PathBuf::from("bin.dat");

    fs.write(
        &text_path,
        bytes::Bytes::from_static(b"plain"),
        WriteOpts::default(),
    )
    .await
    .unwrap();
    fs.write(
        &bin_path,
        bytes::Bytes::from_static(b"x\0y\0z\0bin"),
        WriteOpts::default(),
    )
    .await
    .unwrap();

    let text_meta = fs.stat(&text_path).await.unwrap();
    assert_eq!(text_meta.size, 5);
    assert!(!text_meta.is_binary);

    let bin_meta = fs.stat(&bin_path).await.unwrap();
    assert!(bin_meta.is_binary, "NUL bytes should mark file as binary");
}

#[tokio::test]
async fn exists_distinguishes_present_from_absent() {
    let dir = TempDir::new().unwrap();
    let fs = fs_at(&dir);
    fs.write(
        &PathBuf::from("there.txt"),
        bytes::Bytes::from_static(b"x"),
        WriteOpts::default(),
    )
    .await
    .unwrap();
    assert!(fs.exists(&PathBuf::from("there.txt")).await);
    assert!(!fs.exists(&PathBuf::from("missing.txt")).await);
}

#[tokio::test]
async fn missing_file_maps_to_not_found() {
    let dir = TempDir::new().unwrap();
    let fs = fs_at(&dir);
    let err = fs.stat(&PathBuf::from("nope.txt")).await.unwrap_err();
    assert!(matches!(err, FsError::NotFound(_)), "got {err:?}");
}

#[tokio::test]
async fn boundary_rejects_outside_workspace() {
    let dir = TempDir::new().unwrap();
    let fs = fs_at(&dir);
    let err = fs.stat(&PathBuf::from("../escape.txt")).await.unwrap_err();
    assert!(matches!(err, FsError::OutsideWorkspace(_)), "got {err:?}");
}

#[tokio::test]
async fn list_returns_immediate_children_sorted() {
    let dir = TempDir::new().unwrap();
    let fs = fs_at(&dir);
    for name in ["c.txt", "a.txt", "b.txt"] {
        fs.write(
            &PathBuf::from(name),
            bytes::Bytes::from_static(b"x"),
            WriteOpts::default(),
        )
        .await
        .unwrap();
    }
    let entries = fs.list(&PathBuf::from(".")).await.unwrap();
    let names: Vec<_> = entries.iter().map(|e| e.name.clone()).collect();
    assert_eq!(names, vec!["a.txt", "b.txt", "c.txt"]);
}

#[tokio::test]
async fn glob_respects_gitignore() {
    let dir = TempDir::new().unwrap();
    let fs = fs_at(&dir);
    fs.write(
        &PathBuf::from(".gitignore"),
        bytes::Bytes::from_static(b"ignored.txt\n"),
        WriteOpts::default(),
    )
    .await
    .unwrap();
    fs.write(
        &PathBuf::from("kept.txt"),
        bytes::Bytes::from_static(b"x"),
        WriteOpts::default(),
    )
    .await
    .unwrap();
    fs.write(
        &PathBuf::from("ignored.txt"),
        bytes::Bytes::from_static(b"x"),
        WriteOpts::default(),
    )
    .await
    .unwrap();

    let opts = GlobOpts {
        respect_gitignore: true,
        max_results: 100,
        sort: GlobSort::PathAsc,
    };
    let hits = fs.glob("*.txt", opts).await.unwrap();
    let names: Vec<String> = hits.iter().map(|p| p.display().to_string()).collect();
    assert!(names.iter().any(|n| n.ends_with("kept.txt")));
    assert!(!names.iter().any(|n| n.ends_with("ignored.txt")));
}

#[tokio::test]
async fn grep_matches_regex_with_path_glob() {
    let dir = TempDir::new().unwrap();
    let fs = fs_at(&dir);
    fs.write(
        &PathBuf::from("a.rs"),
        bytes::Bytes::from_static(b"fn alpha() {}\nfn beta() {}\n"),
        WriteOpts::default(),
    )
    .await
    .unwrap();
    fs.write(
        &PathBuf::from("b.txt"),
        bytes::Bytes::from_static(b"fn gamma() {}\n"),
        WriteOpts::default(),
    )
    .await
    .unwrap();

    let hits = fs
        .grep(GrepArgs {
            pattern: "fn \\w+\\(\\)".to_string(),
            path_glob: Some("*.rs".to_string()),
            case_insensitive: false,
            max_hits: 50,
            max_line_chars: 200,
        })
        .await
        .unwrap();
    assert_eq!(hits.len(), 2, "two fn matches in *.rs only");
    for hit in &hits {
        assert!(hit.path.to_string_lossy().ends_with("a.rs"));
    }
}

#[tokio::test]
async fn grep_case_insensitive_flag() {
    let dir = TempDir::new().unwrap();
    let fs = fs_at(&dir);
    fs.write(
        &PathBuf::from("note.txt"),
        bytes::Bytes::from_static(b"Hello\nhello\nHELLO\n"),
        WriteOpts::default(),
    )
    .await
    .unwrap();

    let hits = fs
        .grep(GrepArgs {
            pattern: "hello".to_string(),
            path_glob: None,
            case_insensitive: true,
            max_hits: 50,
            max_line_chars: 200,
        })
        .await
        .unwrap();
    assert_eq!(hits.len(), 3);
}

#[tokio::test]
async fn write_creates_parent_directories() {
    let dir = TempDir::new().unwrap();
    let fs = fs_at(&dir);
    let nested = PathBuf::from("a/b/c/leaf.txt");
    fs.write(
        &nested,
        bytes::Bytes::from_static(b"deep"),
        WriteOpts::default(),
    )
    .await
    .unwrap();
    assert!(fs.exists(&nested).await);
}
