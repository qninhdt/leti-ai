//! Plan-mode agent — read-only profile invoked by `EnterPlanMode`.
//!
//! The system prompt overlay (`plan_mode.md`) tells the model to gather
//! context with read/list/grep/glob, optionally web_search/web_fetch,
//! and finalize via `ExitPlanMode { plan }`. The narrow allowlist is
//! the second filter — even if a plugin registers a write tool, the
//! dispatcher's allowlist check refuses to hand control to it while
//! this agent is active.

use std::sync::Arc;

use openlet_core::agent::{AgentDefinition, AgentSlug, PromptSegments};

const PLAN_CACHEABLE: &str = include_str!("../assets/plan_mode.md");

/// Build the plan-mode agent definition.
#[must_use]
pub fn plan_agent() -> AgentDefinition {
    AgentDefinition {
        slug: AgentSlug::new("plan").expect("static slug"),
        title: "Plan Mode".into(),
        description: "Read-only planning agent — produces a written plan, never edits.".into(),
        prompt_segments: Some(PromptSegments {
            cacheable: PLAN_CACHEABLE.to_owned(),
            dynamic: Arc::new(|_| String::new()),
        }),
        // web_search / web_fetch may not be registered yet (sibling
        // agent owns those tools). Listing them here is harmless: the
        // dispatch-time check filters by *registry presence* AND
        // allowlist, so absent tools just stay absent.
        tool_allowlist: vec![
            "read".into(),
            "list".into(),
            "grep".into(),
            "glob".into(),
            "web_search".into(),
            "web_fetch".into(),
            "enter_plan_mode".into(),
            "exit_plan_mode".into(),
        ],
        model_id: "anthropic/claude-3.5-sonnet".into(),
        default_temperature: 0.0,
        context_window: 200_000,
        compaction_threshold: 0.8,
        compaction_summary_cap_tokens: 2_000,
        hidden: false,
    }
}
