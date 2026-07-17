//! Re-exports of the hook surface from `leti-core`.
//!
//! Hook types live in `leti-core::hooks` so runtime dispatch sites
//! can construct ctx values without a circular dep on this crate.
//! Plugin authors continue to import them via `leti_plugin_api`.

pub use leti_core::hooks::{HookKind, HookResult, Priority, io};
