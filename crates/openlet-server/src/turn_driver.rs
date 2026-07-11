//! Shared turn-driver helpers used by `routes::message::drive_loop` and
//! `subagent_spawner::drive_subagent`.
//!
//! Both functions have ~85% overlap: list messages, project for LLM,
//! materialize tool specs, build read-history, build a `LoopContext` +
//! `TurnInput`, then call `runtime.run_loop`. The diverging bits are:
//!   - projection_caps source (provider caps vs. default)
//!   - permission manager source (state vs. scoped child)
//!   - filesystem source (state.agents[id].fs vs. agent_resources.fs)
//!   - agent_def lookup source (session slug vs. spawn slug)
//!   - mode source (session_meta vs. child session_meta)
//!
//! These shared helpers package up the common pieces; each call site
//! still owns the diverging bits.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use openlet_core::adapters::model_provider::ToolSpec;
use openlet_core::agent::{AgentDefinition, DynamicSegmentInput};
use openlet_core::error::CoreError;
use openlet_core::projection::{LlmMessage, ProjectionCaps, project_for_llm};
use openlet_core::runtime::{LoopContext, RuntimeHandles, TurnInput};
use openlet_core::types::agent::AgentId;
use openlet_core::types::message::MessageId;
use openlet_core::types::part::Part;
use openlet_core::types::session::SessionId;

use crate::app_state::AppState;

/// Bundle of everything `run_loop`/`compact_session` need for a session
/// turn: the assembled loop context, the projected turn input, and the
/// memory-store handle. Built once by [`build_loop_context`] so the normal
/// prompt driver and the on-demand compaction driver share identical setup.
pub(crate) struct LoopSetup {
    pub loop_ctx: LoopContext,
    pub input: TurnInput,
    pub memory: Arc<dyn openlet_core::adapters::memory_store::MemoryStore>,
}

/// Assemble the `LoopContext` + `TurnInput` for a session turn. Resolves the
/// session's model/provider caps, projects its messages, materializes tool
/// specs, and composes the agent system prompt — the shared preamble both
/// `drive_loop` (normal turn) and the compaction route run before calling
/// into the core runtime.
pub(crate) async fn build_loop_context(
    state: &AppState,
    session_id: SessionId,
    agent_id: AgentId,
) -> Result<LoopSetup, CoreError> {
    let agent = state
        .agents
        .get(&agent_id)
        .ok_or(CoreError::Memory(
            openlet_core::error::MemoryError::SessionNotFound,
        ))?
        .clone();

    let session_meta = state
        .memory
        .get_session(session_id)
        .await?
        .ok_or(CoreError::Memory(
            openlet_core::error::MemoryError::SessionNotFound,
        ))?;

    let model = session_meta
        .model
        .clone()
        .unwrap_or_else(|| state.config.default_model.clone());
    let provider_caps = state.provider.capabilities(&model);
    let projection_caps = ProjectionCaps {
        supports_reasoning_replay: false,
        supports_image_input: provider_caps.supports_vision,
        supports_document_input: provider_caps.supports_document_input,
    };
    let llm_messages = project_session(state, session_id, projection_caps).await?;
    let tools = tool_specs(state);
    let read_history = state.read_histories.entry(session_id).or_default().clone();

    let current_slug = session_meta
        .current_agent_slug
        .clone()
        .unwrap_or_else(|| "general".into());
    let agent_def = openlet_core::agent::AgentSlug::new(current_slug)
        .ok()
        .and_then(|slug| state.agent_registry.get(&slug))
        .cloned()
        .map(Arc::new);

    let system_prompt = compose_agent_system_prompt(agent_def.as_ref(), &state.workspace_root);

    let loop_ctx = LoopContext {
        agent_id,
        handles: runtime_handles(state, agent.fs.clone(), state.permission.clone()),
        read_history,
        mode: session_meta.permission_mode,
        max_steps: MAX_TURN_STEPS,
        agent: agent_def,
    };

    let input = build_turn_input(
        session_id,
        llm_messages,
        tools,
        session_meta.model.clone(),
        system_prompt,
    );

    Ok(LoopSetup {
        loop_ctx,
        input,
        memory: memory_arc(state),
    })
}

/// Assemble the `RuntimeHandles` bundle a loop context needs. All ten
/// handles come straight off `AppState` EXCEPT `fs` and `permission`, which
/// differ per driver (session agent fs + shared permission for a top-level
/// turn; scoped child fs + child permission for a subagent). Both drivers
/// route through this so adding a handle is a single-site edit and the two
/// assemblies cannot drift apart.
#[must_use]
pub(crate) fn runtime_handles(
    state: &AppState,
    fs: Arc<dyn openlet_core::adapters::Filesystem>,
    permission: Arc<dyn openlet_core::adapters::permission_manager::PermissionManager>,
) -> RuntimeHandles {
    RuntimeHandles {
        fs,
        permission,
        events: state.events.clone(),
        artifacts: state.artifacts.clone(),
        registry: state.tool_registry.clone(),
        hook_chains: state.hook_chains.clone(),
        questions: state.questions.clone(),
        memory: state.memory.clone(),
        task_registry: state.task_registry.clone(),
        agent_registry: state.agent_registry.clone(),
    }
}

/// Per-turn step ceiling shared by every turn driver (top-level prompt
/// loop + nested subagent loop). Caps runaway tool-call cycles.
pub(crate) const MAX_TURN_STEPS: usize = 50;

/// List a session's messages + parts and project them into LLM-shape.
///
/// Centralises the `list_messages` → `parts_by_msg` HashMap →
/// `project_for_llm` triple every turn driver runs at the top of a
/// loop.
pub(crate) async fn project_session(
    state: &AppState,
    session_id: SessionId,
    caps: ProjectionCaps,
) -> Result<Vec<LlmMessage>, CoreError> {
    let messages = state.memory.list_messages(session_id).await?;
    let mut parts_by_msg: HashMap<MessageId, Vec<Part>> = HashMap::with_capacity(messages.len());
    for m in &messages {
        let parts = state.memory.list_parts(session_id, m.id).await?;
        parts_by_msg.insert(m.id, parts);
    }
    Ok(project_for_llm(&messages, &parts_by_msg, caps))
}

/// Materialise the active tool registry as the `Vec<ToolSpec>` shape
/// the model provider's `chat_stream` consumes. Drops the
/// `(name, handle)` indirection every driver previously rebuilt by
/// hand.
#[must_use]
pub(crate) fn tool_specs(state: &AppState) -> Vec<ToolSpec> {
    state
        .tool_registry
        .iter()
        .map(|(name, handle)| ToolSpec {
            name: name.to_string(),
            description: handle.description().to_string(),
            parameters: handle.input_schema(),
        })
        .collect()
}

/// Bind the `Arc<dyn MemoryStore>` view the runtime expects without
/// each caller writing the explicit type ascription.
#[must_use]
pub(crate) fn memory_arc(
    state: &AppState,
) -> Arc<dyn openlet_core::adapters::memory_store::MemoryStore> {
    state.memory.clone()
}

/// Assemble the `TurnInput` both drivers feed into `run_loop`. `model`
/// is the session's per-session override (`None` ⇒ the runtime resolves
/// from `RuntimeConfig::default_model`). `system_prompt` is the agent's
/// composed identity+guidance prompt (see [`compose_agent_system_prompt`]);
/// the runtime appends the per-provider overlay after it. `max_tokens` /
/// `temperature` default to `None`.
#[must_use]
pub(crate) fn build_turn_input(
    session_id: SessionId,
    messages: Vec<LlmMessage>,
    tools: Vec<ToolSpec>,
    model: Option<String>,
    system_prompt: Option<String>,
) -> TurnInput {
    TurnInput {
        session_id,
        messages,
        system_prompt,
        model,
        max_tokens: None,
        temperature: None,
        tools,
    }
}

/// Compose an agent's system prompt from its two-part `PromptSegments`:
/// the cacheable identity/guidance block (placed first for prompt-cache
/// stability) followed by the per-turn dynamic segment (workspace path +
/// date). Returns `None` when the agent has no prompt segments, so the
/// runtime falls back to the provider overlay alone.
///
/// Without this, the top-level turn sent `system_prompt: None` and the
/// agent never learned its name, mission, or tool catalog — the rich
/// `general_cacheable.md` was dead code.
#[must_use]
pub(crate) fn compose_agent_system_prompt(
    agent_def: Option<&Arc<AgentDefinition>>,
    workspace_root: &Path,
) -> Option<String> {
    let segments = agent_def?.prompt_segments.as_ref()?;
    let dynamic = (segments.dynamic)(&DynamicSegmentInput {
        workspace_root: workspace_root.to_path_buf(),
        now: chrono::Utc::now(),
    });
    let cacheable = segments.cacheable.trim_end();
    if cacheable.is_empty() && dynamic.trim().is_empty() {
        return None;
    }
    if dynamic.trim().is_empty() {
        Some(cacheable.to_string())
    } else {
        Some(format!("{cacheable}\n\n{dynamic}"))
    }
}

#[cfg(test)]
mod compose_prompt_tests {
    use super::*;
    use openlet_core::agent::{AgentDefinition, AgentSlug, PromptSegments};
    use std::path::PathBuf;

    fn agent_with_segments(segments: Option<PromptSegments>) -> Arc<AgentDefinition> {
        Arc::new(AgentDefinition {
            slug: AgentSlug::new("general").unwrap(),
            title: "General".into(),
            description: String::new(),
            prompt_segments: segments,
            tool_allowlist: Vec::new(),
            model_id: None,
            default_temperature: 0.0,
            context_window: 1000,
            compaction_threshold: 0.8,
            compaction_summary_cap_tokens: 500,
            hidden: false,
        })
    }

    #[test]
    fn none_agent_yields_no_prompt() {
        assert!(compose_agent_system_prompt(None, &PathBuf::from("/ws")).is_none());
    }

    #[test]
    fn agent_without_segments_yields_no_prompt() {
        let agent = agent_with_segments(None);
        assert!(compose_agent_system_prompt(Some(&agent), &PathBuf::from("/ws")).is_none());
    }

    #[test]
    fn cacheable_and_dynamic_are_joined() {
        let agent = agent_with_segments(Some(PromptSegments {
            cacheable: "You are the test agent.".into(),
            dynamic: Arc::new(|input| format!("Workspace: {}", input.workspace_root.display())),
        }));
        let out = compose_agent_system_prompt(Some(&agent), &PathBuf::from("/ws")).unwrap();
        assert!(out.starts_with("You are the test agent."));
        assert!(out.contains("Workspace: /ws"));
    }

    #[test]
    fn empty_dynamic_yields_cacheable_alone() {
        let agent = agent_with_segments(Some(PromptSegments {
            cacheable: "Only cacheable.".into(),
            dynamic: Arc::new(|_| String::new()),
        }));
        let out = compose_agent_system_prompt(Some(&agent), &PathBuf::from("/ws")).unwrap();
        assert_eq!(out, "Only cacheable.");
    }
}
