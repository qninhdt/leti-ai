//! Read / write / stat / list ops on a workspace-rooted local FS.
//!
//! Split into:
//! - `meta`  — `mtime_ms` helper shared by read+write
//! - `read`  — `read`, `stat`, `exists`, `list`, `sniff_binary`
//! - `write` — `write` + atomic write helpers

mod meta;
mod read;
mod write;

pub(crate) use read::{exists, list, read, stat};
pub(crate) use write::write;
