//! Config-driven `PermissionManager` impl.
//!
//! Last-match-wins ruleset plus a pending-ask map keyed by `AskId`.
//! Layered ruleset (defaults ++ agent ++ workspace ++ session) lands when
//! agent definitions are plumbed; the current layer ships a single layer +
//! the interactive-ask flow.

mod manager;
#[cfg(test)]
mod manager_tests;
mod matcher;
mod ruleset;

pub use manager::ConfigPermissionMgr;
pub use matcher::build_permission_subject;
