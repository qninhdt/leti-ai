//! Small cross-adapter helpers shared by multiple adapter modules.

/// Floor `index` to the nearest UTF-8 char boundary at or below it.
///
/// Slicing a `&str` at a non-boundary byte index panics; this returns a
/// safe cut point for any `index <= s.len()`. Equivalent of the nightly
/// `str::floor_char_boundary`. Consolidates the five byte-identical
/// re-implementations that previously lived in `cloudfs/rematch.rs`,
/// `openai/transport.rs`, `pyexec/executor.rs`, `emushell/eval.rs`, and
/// `localfs/filesystem/walk.rs`.
#[must_use]
pub(crate) fn floor_char_boundary(s: &str, mut index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(index) {
        index -= 1;
    }
    index
}
