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

use std::path::Path;
use std::sync::Arc;

use leti_core::adapters::model_provider::ToolSpec;
use leti_core::agent::{AgentDefinition, DynamicSegmentInput};
use leti_core::error::CoreError;
use leti_core::projection::{LlmMessage, ProjectionCaps};
use leti_core::runtime::{LoopContext, RuntimeHandles, TurnInput};
use leti_core::types::agent::AgentId;
use leti_core::types::session::SessionId;

use crate::app_state::AppState;

/// Bundle of everything `run_loop`/`compact_session` need for a session
/// turn: the assembled loop context, the projected turn input, and the
/// memory-store handle. Built once by [`build_loop_context`] so the normal
/// prompt driver and the on-demand compaction driver share identical setup.
pub(crate) struct LoopSetup {
    pub loop_ctx: LoopContext,
    pub input: TurnInput,
    pub memory: Arc<dyn leti_core::adapters::memory_store::MemoryStore>,
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
            leti_core::error::MemoryError::SessionNotFound,
        ))?
        .clone();

    let session_meta = state
        .memory
        .get_session(session_id)
        .await?
        .ok_or(CoreError::Memory(
            leti_core::error::MemoryError::SessionNotFound,
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
    let handles = runtime_handles(state, agent.fs.clone(), state.permission.clone());
    let llm_messages = leti_core::runtime::prepare_session_messages(
        &handles,
        session_id,
        projection_caps,
        leti_core::runtime::ReminderRequestContext::default(),
    )
    .await?;
    let tools = tool_specs(state);
    let read_history = state.read_histories.entry(session_id).or_default().clone();

    let current_slug = session_meta
        .current_agent_slug
        .clone()
        .unwrap_or_else(|| "general".into());
    let agent_def = leti_core::agent::AgentSlug::new(current_slug)
        .ok()
        .and_then(|slug| state.agent_registry.get(&slug))
        .cloned()
        .map(Arc::new);

    let system_prompt = compose_agent_system_prompt(agent_def.as_ref(), &state.workspace_root);

    let loop_ctx = LoopContext {
        agent_id,
        handles,
        read_history,
        mode: session_meta.permission_mode,
        interaction_mode: session_meta.interaction_mode,
        max_steps: MAX_TURN_STEPS,
        projection_caps,
        agent: agent_def,
        ext: Default::default(),
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
    fs: Arc<dyn leti_core::adapters::Filesystem>,
    permission: Arc<dyn leti_core::adapters::permission_manager::PermissionManager>,
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
        tool_scheduler: state.tool_scheduler.clone(),
    }
}

/// Per-turn step ceiling shared by every turn driver (top-level prompt
/// loop + nested subagent loop). Caps runaway tool-call cycles.
pub(crate) const MAX_TURN_STEPS: usize = 50;

/// Materialise the active tool registry as the `Vec<ToolSpec>` shape
/// the model provider's `chat_stream` consumes. Drops the
/// `(name, handle)` indirection every driver previously rebuilt by
/// hand.
#[must_use]
pub(crate) fn tool_specs(state: &AppState) -> Vec<ToolSpec> {
    state
        .tool_registry
        .iter()
        .map(|(name, handle)| {
            let mut spec = ToolSpec {
                name: name.to_string(),
                description: handle.description().to_string(),
                parameters: handle.input_schema(),
            };
            if name == "subagent_task" {
                add_agent_catalog(&mut spec, &state.agent_registry);
            }
            spec
        })
        .collect()
}

fn add_agent_catalog(spec: &mut ToolSpec, agents: &leti_core::agent::AgentRegistry) {
    let mut catalog = agents
        .iter_visible()
        .map(|(slug, definition)| {
            (
                slug.as_str().to_string(),
                definition.description.trim().to_string(),
            )
        })
        .collect::<Vec<_>>();
    catalog.sort_by(|a, b| a.0.cmp(&b.0));

    let slugs = catalog
        .iter()
        .map(|(slug, _)| serde_json::Value::String(slug.clone()))
        .collect::<Vec<_>>();
    if let Some(property) = spec
        .parameters
        .pointer_mut("/properties/subagent_type")
        .and_then(serde_json::Value::as_object_mut)
    {
        property.insert("enum".into(), serde_json::Value::Array(slugs));
        property.insert(
            "description".into(),
            serde_json::Value::String(
                "Exact registered agent slug. Omit to use general; never invent a slug.".into(),
            ),
        );
    }

    let lines = catalog
        .iter()
        .map(|(slug, description)| {
            if description.is_empty() {
                format!("- {slug}")
            } else {
                format!("- {slug}: {description}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    spec.description.push_str(
        "\n\nSelect subagent_type only from the exact registered slugs below, based on each description. Words in the user's request such as 'scout', 'researcher', or 'reviewer' describe the job; they are not agent slugs unless listed below. Do not copy an unlisted role word into subagent_type. If no specialized agent matches, omit subagent_type so the general agent handles the task. Do not refuse delegation merely because the user's role label is not registered.\nAvailable agent types:\n",
    );
    spec.description.push_str(&lines);
}

/// Bind the `Arc<dyn MemoryStore>` view the runtime expects without
/// each caller writing the explicit type ascription.
#[must_use]
pub(crate) fn memory_arc(
    state: &AppState,
) -> Arc<dyn leti_core::adapters::memory_store::MemoryStore> {
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
    use async_trait::async_trait;
    use leti_core::adapters::model_provider::ToolSpec;
    use leti_core::adapters::tool_executor::ToolCtx;
    use leti_core::agent::{AgentDefinition, AgentRegistry, AgentSlug, PromptSegments};
    use leti_core::runtime::subagent::{SpawnError, TaskId, TaskStatus};
    use leti_core::tools::ErasedTool;
    use leti_core::tools::builtins::subagent_task::{SubagentSpawner, SubagentTaskTool};
    use serde_json::json;
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

    #[test]
    fn subagent_tool_catalog_lists_visible_agents_and_constrains_schema() {
        struct NeverSpawner;

        #[async_trait]
        impl SubagentSpawner for NeverSpawner {
            async fn spawn(
                &self,
                _: &ToolCtx,
                _: &str,
                _: &str,
                _: Option<&str>,
                _: bool,
            ) -> Result<leti_core::tools::builtins::subagent_task::SpawnedSubagent, SpawnError>
            {
                unreachable!("schema test does not execute the tool")
            }

            async fn await_completion(
                &self,
                _: TaskId,
            ) -> Result<(String, Option<String>, TaskStatus), SpawnError> {
                unreachable!("schema test does not execute the tool")
            }
        }

        let mut agents = AgentRegistry::new();
        let mut general = (*agent_with_segments(None)).clone();
        general.description = "General-purpose delegation".into();
        agents.insert(general).unwrap();

        let mut indexer = (*agent_with_segments(None)).clone();
        indexer.slug = AgentSlug::new("indexer").unwrap();
        indexer.title = "Indexer".into();
        indexer.description = "Inspect repository structure".into();
        agents.insert(indexer).unwrap();

        let mut hidden = (*agent_with_segments(None)).clone();
        hidden.slug = AgentSlug::new("internal").unwrap();
        hidden.hidden = true;
        agents.insert(hidden).unwrap();

        let mut spec = ToolSpec {
            name: "subagent_task".into(),
            description: SubagentTaskTool::new(Arc::new(NeverSpawner))
                .description()
                .to_string(),
            parameters: ErasedTool::input_schema(&SubagentTaskTool::new(Arc::new(NeverSpawner))),
        };
        add_agent_catalog(&mut spec, &agents);

        assert!(
            spec.description
                .contains("- general: General-purpose delegation")
        );
        assert!(
            spec.description
                .contains("- indexer: Inspect repository structure")
        );
        assert!(spec.description.contains(
            "Words in the user's request such as 'scout', 'researcher', or 'reviewer' describe the job"
        ));
        assert!(spec.description.contains(
            "If no specialized agent matches, omit subagent_type so the general agent handles the task"
        ));
        assert!(!spec.description.contains("internal"));
        assert_eq!(
            spec.parameters.pointer("/properties/subagent_type/enum"),
            Some(&json!(["general", "indexer"]))
        );
        assert_eq!(
            spec.parameters.pointer("/required"),
            Some(&json!(["objective"]))
        );
    }
}
