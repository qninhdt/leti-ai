//! Indexer reference custom agent. Stub-grade — proves the registration
//! flow. Real indexing is post-MVP per the brainstorm.

use std::sync::Arc;

use openlet_core::agent::{AgentDefinition, AgentSlug, PromptSegments};

const INDEXER_CACHEABLE: &str = "You are the Openlet workspace indexer.\n\
For MVP this agent only logs and returns 'not yet implemented'.\n\
Real indexing of code symbols, embeddings, and references lands post-MVP.\n";

#[must_use]
pub fn indexer_agent() -> AgentDefinition {
    AgentDefinition {
        slug: AgentSlug::new("indexer").expect("static slug"),
        title: "Workspace Indexer (stub)".into(),
        description: "Reference custom agent — read-only, returns a stub response.".into(),
        prompt_segments: Some(PromptSegments {
            cacheable: INDEXER_CACHEABLE.to_owned(),
            dynamic: Arc::new(|_| String::new()),
        }),
        tool_allowlist: vec!["read".into(), "list".into(), "glob".into()],
        model_id: "anthropic/claude-3.5-haiku".into(),
        default_temperature: 0.0,
        context_window: 200_000,
        compaction_threshold: 0.8,
        compaction_summary_cap_tokens: 2_000,
        hidden: false,
        max_cost_per_session_usd: None,
    }
}
