//! `AgentDefinition` — the behavior an agent exposes.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::slug::AgentSlug;

/// Function building the dynamic prompt segment per turn (timestamp,
/// workspace path, current cwd). Kept as `Arc<dyn Fn>` so plugins can pass
/// closures that capture per-plugin context.
pub type DynamicSegmentFn = Arc<dyn Fn(&DynamicSegmentInput) -> String + Send + Sync>;

/// Input passed to `DynamicSegmentFn` per turn.
#[derive(Debug, Clone)]
pub struct DynamicSegmentInput {
    pub workspace_root: std::path::PathBuf,
    pub now: chrono::DateTime<chrono::Utc>,
}

/// Two-part prompt: `cacheable` is hashed for the prompt-cache lock
/// (amendment §R) and placed FIRST in the system message so Anthropic
/// prompt cache hits. `dynamic` runs each turn and is appended after.
#[derive(Clone)]
pub struct PromptSegments {
    pub cacheable: String,
    pub dynamic: DynamicSegmentFn,
}

impl std::fmt::Debug for PromptSegments {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PromptSegments")
            .field("cacheable_len", &self.cacheable.len())
            .field("dynamic", &"<fn>")
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    pub slug: AgentSlug,
    pub title: String,
    pub description: String,
    #[serde(skip)]
    pub prompt_segments: Option<PromptSegments>,
    /// Tool names the agent may invoke (gating happens in `ConfigPermissionMgr`
    /// — this list is the *first* filter; permission rules are the second).
    pub tool_allowlist: Vec<String>,
    pub model_id: String,
    pub default_temperature: f32,
    pub context_window: u32,
    /// Fraction of `context_window` at which compaction triggers. Default 0.8.
    pub compaction_threshold: f32,
    /// Compaction summary cap; default 2 KB tokens (≈8 KB chars).
    pub compaction_summary_cap_tokens: u32,
    /// Hidden from the public agent listing — used for built-ins like
    /// `compaction-summarizer` once we add a separate compaction agent.
    #[serde(default)]
    pub hidden: bool,
}

impl AgentDefinition {
    #[must_use]
    pub fn cacheable_prompt(&self) -> &str {
        self.prompt_segments
            .as_ref()
            .map(|s| s.cacheable.as_str())
            .unwrap_or_default()
    }
}

impl Default for PromptSegments {
    fn default() -> Self {
        Self {
            cacheable: String::new(),
            dynamic: Arc::new(|_| String::new()),
        }
    }
}
