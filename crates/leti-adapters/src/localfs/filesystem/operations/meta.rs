//! Small metadata helpers shared by `read` and `write` ops.

use std::fs::Metadata;
use std::time::UNIX_EPOCH;

/// Extract `mtime` as ms-since-epoch from a `Metadata`. Returns `0`
/// when the platform doesn't expose modification time or the value is
/// pre-epoch / out of `i64` range — same fallback both `stat` and
/// `write` had inline before this was extracted.
pub(super) fn mtime_ms(meta: &Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}
