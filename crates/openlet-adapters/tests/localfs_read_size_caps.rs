//! Phase 4 — `LocalFilesystem::read` size caps + range edge cases.
//!
//! `MAX_READ_BYTES = 8 MiB` is the hard floor in
//! `localfs/filesystem/operations/read.rs:19`. The tool layer caps
//! tighter; this file exercises the floor under TOCTOU growth and
//! checks range-edge contracts.

mod common;

use std::path::Path;

use bytes::Bytes;
use common::tempdir_workspace::WorkspaceFixture;
use openlet_adapters::localfs::LocalFilesystem;
use openlet_core::adapters::filesystem::{ByteRange, Filesystem, WriteOpts};
use openlet_core::error::FsError;

const MAX_READ_BYTES: u64 = 8 * 1024 * 1024;

async fn write_padded_file(fs: &LocalFilesystem, name: &Path, size: usize) {
    let bytes = Bytes::from(vec![b'x'; size]);
    fs.write(name, bytes, WriteOpts::default())
        .await
        .expect("seed file");
}

#[tokio::test]
async fn read_with_no_range_caps_at_max_read_bytes_for_oversize_file() {
    let fx = WorkspaceFixture::empty();
    let fs = LocalFilesystem::new(fx.root().to_path_buf());
    // 8 MiB + 1 byte file
    let path = Path::new("oversize.bin");
    write_padded_file(&fs, path, MAX_READ_BYTES as usize + 1).await;

    let bytes = fs.read(path, None).await.expect("read");
    assert_eq!(
        bytes.len() as u64,
        MAX_READ_BYTES,
        "no-range read MUST be capped at MAX_READ_BYTES (got {})",
        bytes.len()
    );
}

#[tokio::test]
async fn read_with_range_start_past_eof_returns_invalid_input() {
    let fx = WorkspaceFixture::with_files(vec![("small.txt", "hello")]);
    let fs = LocalFilesystem::new(fx.root().to_path_buf());
    let err = fs
        .read(
            Path::new("small.txt"),
            Some(ByteRange { start: 100, len: 0 }),
        )
        .await
        .expect_err("range start past eof must error");
    assert!(matches!(err, FsError::InvalidInput(_)), "got {err:?}");
}

#[tokio::test]
async fn read_with_zero_len_reads_to_eof() {
    let fx = WorkspaceFixture::with_files(vec![("hello.txt", "0123456789")]);
    let fs = LocalFilesystem::new(fx.root().to_path_buf());
    let bytes = fs
        .read(Path::new("hello.txt"), Some(ByteRange { start: 3, len: 0 }))
        .await
        .expect("read");
    assert_eq!(&bytes[..], b"3456789");
}

#[tokio::test]
async fn read_with_explicit_len_clamps_at_eof() {
    let fx = WorkspaceFixture::with_files(vec![("a.txt", "abcde")]);
    let fs = LocalFilesystem::new(fx.root().to_path_buf());
    let bytes = fs
        .read(
            Path::new("a.txt"),
            Some(ByteRange {
                start: 2,
                len: 1000,
            }),
        )
        .await
        .expect("read");
    // Only 3 bytes available from offset 2; len clamps to total - start.
    assert_eq!(&bytes[..], b"cde");
}

#[tokio::test]
async fn read_caps_at_max_even_when_range_len_is_huge() {
    // Caller asks for 100 MiB from a 16 MiB file. The (total - start)
    // clamp brings len to 16 MiB; then `len.min(MAX_READ_BYTES)`
    // reduces it to 8 MiB.
    let fx = WorkspaceFixture::empty();
    let fs = LocalFilesystem::new(fx.root().to_path_buf());
    let path = Path::new("big.bin");
    write_padded_file(&fs, path, 16 * 1024 * 1024).await;
    let bytes = fs
        .read(
            path,
            Some(ByteRange {
                start: 0,
                len: 100 * 1024 * 1024,
            }),
        )
        .await
        .expect("read");
    assert_eq!(bytes.len() as u64, MAX_READ_BYTES);
}
