//! General assistant agent definition.

use std::sync::Arc;

use leti_core::agent::{AgentDefinition, DynamicSegmentInput};

use crate::builder::{AgentBlueprint, build};

/// Cacheable system prompt for the general agent. Hashed in
/// `tests/prompt_cache_hash.rs` — silently editing this
/// file invalidates the prompt cache for every active session, so the
/// `# version: N` header must increment alongside changes.
pub const GENERAL_CACHEABLE: &str = include_str!("general_cacheable.md");

const TOOL_ALLOWLIST: &[&str] = &[
    "read", "list", "glob", "grep", "write", "edit", "bash", "todo",
];

/// Build the general agent definition.
#[must_use]
pub fn general_agent() -> AgentDefinition {
    build(AgentBlueprint {
        slug: "general",
        title: "General Assistant",
        description: "Default coding-aware agent with the full tool catalog.",
        cacheable: GENERAL_CACHEABLE.to_owned(),
        dynamic: Arc::new(dynamic_segment),
        tool_allowlist: TOOL_ALLOWLIST,
    })
}

fn dynamic_segment(input: &DynamicSegmentInput) -> String {
    format!(
        "Workspace: {}\nDate: {}\n",
        input.workspace_root.display(),
        input.now.format("%Y-%m-%d"),
    )
}
