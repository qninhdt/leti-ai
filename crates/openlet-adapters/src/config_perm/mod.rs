//! Config-driven `PermissionManager` impl.
//!
//! Last-match-wins ruleset (we diverge from claw-code's first-match)
//! plus a pending-ask map keyed by `AskId`. Layered ruleset (defaults
//! ++ agent ++ workspace ++ session) per amendment §E lands when phase 4
//! plumbs agent definitions; phase 4A ships a single layer + the
//! interactive-ask flow.

mod manager;
mod matcher;
mod ruleset;

pub use manager::ConfigPermissionMgr;
pub use matcher::build_permission_subject;
