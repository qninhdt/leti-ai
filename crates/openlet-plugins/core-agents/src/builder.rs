//! Shared constructor for the built-in core agents.
//!
//! `general`, `indexer`, and `plan` share identical window / compaction /
//! temperature tuning. Centralizing it here keeps a single source of truth
//! and removes the triplicated `AgentDefinition` struct literals — only the
//! distinguishing fields (slug, prompt, allowlist, model) live at each call
//! site.

use openlet_core::agent::{AgentDefinition, AgentSlug, DynamicSegmentFn, PromptSegments};

/// Context window shared by every built-in agent (tokens).
const CONTEXT_WINDOW: u32 = 200_000;
/// Fraction of `CONTEXT_WINDOW` at which compaction triggers.
const COMPACTION_THRESHOLD: f32 = 0.8;
/// Compaction summary cap (≈8 KB chars at 2 K tokens).
const COMPACTION_SUMMARY_CAP_TOKENS: u32 = 2_000;
/// Deterministic-by-default sampling for coding agents.
const DEFAULT_TEMPERATURE: f32 = 0.0;

/// Distinguishing fields for a built-in agent. Shared tuning is filled in
/// by [`build`].
pub(crate) struct AgentBlueprint {
    pub slug: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub cacheable: String,
    pub dynamic: DynamicSegmentFn,
    pub tool_allowlist: &'static [&'static str],
    pub model_id: &'static str,
}

/// Assemble an [`AgentDefinition`] from a blueprint, applying the tuning
/// constants every built-in agent shares.
pub(crate) fn build(bp: AgentBlueprint) -> AgentDefinition {
    let def = AgentDefinition {
        slug: AgentSlug::new(bp.slug).expect("static slug"),
        title: bp.title.into(),
        description: bp.description.into(),
        prompt_segments: Some(PromptSegments {
            cacheable: bp.cacheable,
            dynamic: bp.dynamic,
        }),
        tool_allowlist: bp.tool_allowlist.iter().map(|s| (*s).to_owned()).collect(),
        model_id: bp.model_id.into(),
        default_temperature: DEFAULT_TEMPERATURE,
        context_window: CONTEXT_WINDOW,
        compaction_threshold: COMPACTION_THRESHOLD,
        compaction_summary_cap_tokens: COMPACTION_SUMMARY_CAP_TOKENS,
        hidden: false,
    };
    // M4 — reject malformed numeric tuning at load time. The shared
    // constants above are valid, so this only fires if a future edit breaks
    // the invariant — fail loudly at boot rather than silently mis-compacting.
    def.validate().expect("built-in agent definition invalid");
    def
}
