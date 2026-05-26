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
use std::sync::Arc;

use openlet_core::adapters::model_provider::ToolSpec;
use openlet_core::error::CoreError;
use openlet_core::projection::{LlmMessage, ProjectionCaps, project_for_llm};
use openlet_core::types::message::MessageId;
use openlet_core::types::part::Part;
use openlet_core::types::session::SessionId;

use crate::app_state::AppState;

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
