//! Pure conversion / normalization helpers for the cloud filesystem.
//!
//! No network, no `CloudFilesystem` state — just path normalization, name
//! disambiguation, and gRPC↔trait type mapping. Split out of `mod.rs` so the
//! struct + trait impl stay focused on the RPC dance.

use std::path::Path;

use openlet_core::adapters::filesystem::FileMeta;
use openlet_core::error::FsError;
use tonic::Status;

use super::pb;

/// Find the single entry in `items` whose name equals `want`, returning its
/// mapped value. Errors `NotFound` when absent and `InvalidInput` when the
/// backend returns MORE THAN ONE match (ambiguous — resolving to an arbitrary
/// row could target the wrong file; reviewer M4). `rel` is the original path
/// for error context.
pub(super) fn unique_named<T, N, V>(
    items: &[T],
    want: &str,
    name_of: N,
    value_of: V,
    rel: &str,
) -> Result<String, FsError>
where
    N: Fn(&T) -> &str,
    V: Fn(&T) -> String,
{
    let mut matches = items.iter().filter(|it| name_of(it) == want);
    let Some(first) = matches.next() else {
        return Err(FsError::NotFound(rel.to_string()));
    };
    if matches.next().is_some() {
        return Err(FsError::InvalidInput(format!(
            "ambiguous path '{rel}': multiple entries named '{want}' in one folder"
        )));
    }
    Ok(value_of(first))
}

/// Normalize a workspace-relative path to a forward-slash string, rejecting
/// absolute paths and `..` escapes (workspace-boundary invariant the trait
/// imposes on every impl).
pub(super) fn normalize_rel(path: &Path) -> Result<String, FsError> {
    if path.is_absolute() {
        return Err(FsError::OutsideWorkspace(path.display().to_string()));
    }
    let mut parts: Vec<String> = Vec::new();
    for comp in path.components() {
        use std::path::Component;
        match comp {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(FsError::OutsideWorkspace(path.display().to_string()));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(FsError::OutsideWorkspace(path.display().to_string()));
            }
        }
    }
    Ok(parts.join("/"))
}

/// Map a `FileInfo` to trait `FileMeta`. `mtime_ms` from `updated_at`;
/// `is_binary` inferred from the magika label (text vs not).
pub(super) fn file_info_to_meta(info: &pb::FileInfo) -> FileMeta {
    let mtime_ms = info
        .updated_at
        .as_ref()
        .map(|t| t.seconds * 1000 + i64::from(t.nanos) / 1_000_000)
        .unwrap_or(0);
    // Heuristic parity with local `is_binary`: treat non-text magika labels as
    // binary. file-service classifies via Magika at extraction time.
    let is_binary = !info.magika_label.is_empty()
        && !info.magika_label.starts_with("text")
        && info.magika_label != "txt"
        && info.magika_label != "markdown"
        && info.magika_label != "code";
    FileMeta {
        size: u64::try_from(info.size_bytes).unwrap_or(0),
        mtime_ms,
        is_binary,
        sha256: None,
    }
}

/// Map a gRPC `Status` to an `FsError`. `NotFound` → `FsError::NotFound`;
/// everything else (including `Unimplemented` from a not-yet-deployed backend)
/// → `FsError::Io` carrying the code + message.
pub(super) fn status_to_fs(s: Status) -> FsError {
    match s.code() {
        tonic::Code::NotFound => FsError::NotFound(s.message().to_string()),
        tonic::Code::InvalidArgument => FsError::InvalidInput(s.message().to_string()),
        tonic::Code::Unimplemented => FsError::Io(format!(
            "file-service RPC unimplemented (deploy skew? backend must ship the \
             GrepFiles/proto revision before cloud mode enables): {}",
            s.message()
        )),
        code => FsError::Io(format!("file-service gRPC {code:?}: {}", s.message())),
    }
}
