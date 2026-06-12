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
/// and placed FIRST in the system message so Anthropic
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
    pub model_id: Option<String>,
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

    /// Validate numeric invariants the runtime relies on. Call this at
    /// load time (builder / future from-toml path) so a malformed
    /// `compaction_threshold` is rejected up front rather than silently
    /// producing a degenerate compaction limit (a negative or NaN threshold
    /// makes `context_window * threshold` zero/NaN, which would either
    /// compact on every turn or never).
    ///
    /// Contract: `compaction_threshold` must be a real number in `(0.0, 1.0]`.
    pub fn validate(&self) -> Result<(), String> {
        let t = self.compaction_threshold;
        if t.is_nan() {
            return Err(format!(
                "agent '{}': compaction_threshold is NaN (must be in (0.0, 1.0])",
                self.slug.as_str()
            ));
        }
        if !(t > 0.0 && t <= 1.0) {
            return Err(format!(
                "agent '{}': compaction_threshold {t} out of range (must be in (0.0, 1.0])",
                self.slug.as_str()
            ));
        }
        Ok(())
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

#[cfg(test)]
mod validate_tests {
    //! `compaction_threshold` must be a real number in `(0.0, 1.0]`.
    use super::*;

    fn agent_with_threshold(t: f32) -> AgentDefinition {
        AgentDefinition {
            slug: AgentSlug::new("general").unwrap(),
            title: "General".into(),
            description: String::new(),
            prompt_segments: Some(PromptSegments::default()),
            tool_allowlist: Vec::new(),
            model_id: Some("test/model".into()),
            default_temperature: 0.0,
            context_window: 1000,
            compaction_threshold: t,
            compaction_summary_cap_tokens: 500,
            hidden: false,
        }
    }

    #[test]
    fn accepts_valid_threshold() {
        assert!(agent_with_threshold(0.8).validate().is_ok());
        assert!(
            agent_with_threshold(1.0).validate().is_ok(),
            "1.0 inclusive"
        );
        assert!(
            agent_with_threshold(f32::MIN_POSITIVE).validate().is_ok(),
            "smallest positive is valid"
        );
    }

    #[test]
    fn rejects_negative_threshold() {
        let err = agent_with_threshold(-0.5).validate().unwrap_err();
        assert!(err.contains("out of range"), "got: {err}");
    }

    #[test]
    fn rejects_zero_threshold() {
        // 0.0 would make the limit 0 → compact on every turn. Reject.
        assert!(agent_with_threshold(0.0).validate().is_err());
    }

    #[test]
    fn rejects_above_one_threshold() {
        assert!(agent_with_threshold(1.5).validate().is_err());
    }

    #[test]
    fn rejects_nan_threshold() {
        let err = agent_with_threshold(f32::NAN).validate().unwrap_err();
        assert!(err.contains("NaN"), "got: {err}");
    }
}
