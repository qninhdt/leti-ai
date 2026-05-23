//! General assistant agent definition.

use std::sync::Arc;

use openlet_core::agent::{AgentDefinition, AgentSlug, DynamicSegmentInput, PromptSegments};

/// Cacheable system prompt for the general agent. Hashed in
/// `tests/prompt_cache_hash.rs` (amendment §R) — silently editing this
/// file invalidates the prompt cache for every active session, so the
/// `# version: N` header must increment alongside changes.
pub const GENERAL_CACHEABLE: &str = include_str!("general_cacheable.md");

/// Build the general agent definition.
#[must_use]
pub fn general_agent() -> AgentDefinition {
    AgentDefinition {
        slug: AgentSlug::new("general").expect("static slug"),
        title: "General Assistant".into(),
        description: "Default coding-aware agent with the full tool catalog.".into(),
        prompt_segments: Some(PromptSegments {
            cacheable: GENERAL_CACHEABLE.to_owned(),
            dynamic: Arc::new(dynamic_segment),
        }),
        tool_allowlist: vec![
            "read".into(),
            "list".into(),
            "glob".into(),
            "grep".into(),
            "write".into(),
            "edit".into(),
            "bash".into(),
            "todo".into(),
        ],
        model_id: "anthropic/claude-3.5-sonnet".into(),
        default_temperature: 0.0,
        context_window: 200_000,
        compaction_threshold: 0.8,
        compaction_summary_cap_tokens: 2_000,
        hidden: false,
        max_cost_per_session_usd: None,
    }
}

fn dynamic_segment(input: &DynamicSegmentInput) -> String {
    format!(
        "Workspace: {}\nDate: {}\n",
        input.workspace_root.display(),
        input.now.format("%Y-%m-%d"),
    )
}
