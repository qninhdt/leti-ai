//! Plan-mode agent — read-only profile invoked by `EnterPlanMode`.
//!
//! The system prompt overlay (`plan_mode.md`) tells the model to gather
//! context with read/list/grep/glob, optionally web_search/web_fetch,
//! and finalize via `ExitPlanMode { plan }`. The narrow allowlist is
//! the second filter — even if a plugin registers a write tool, the
//! dispatcher's allowlist check refuses to hand control to it while
//! this agent is active.

use std::sync::Arc;

use openlet_core::agent::AgentDefinition;

use crate::builder::{AgentBlueprint, build};

/// Cacheable system prompt for the plan-mode agent. Hashed in
/// `tests/plan_prompt_cache_hash.rs` — silently editing this
/// file invalidates the prompt cache for every active session, so the
/// `# version: N` header must increment alongside changes.
pub const PLAN_CACHEABLE: &str = include_str!("../assets/plan_mode.md");

// web_search / web_fetch may not be registered yet (sibling agent owns
// those tools). Listing them here is harmless: the dispatch-time check
// filters by *registry presence* AND allowlist, so absent tools just
// stay absent.
const TOOL_ALLOWLIST: &[&str] = &[
    "read",
    "list",
    "grep",
    "glob",
    "web_search",
    "web_fetch",
    "enter_plan_mode",
    "exit_plan_mode",
];

/// Build the plan-mode agent definition.
#[must_use]
pub fn plan_agent() -> AgentDefinition {
    build(AgentBlueprint {
        slug: "plan",
        title: "Plan Mode",
        description: "Read-only planning agent — produces a written plan, never edits.",
        cacheable: PLAN_CACHEABLE.to_owned(),
        dynamic: Arc::new(|_| String::new()),
        tool_allowlist: TOOL_ALLOWLIST,
    })
}
