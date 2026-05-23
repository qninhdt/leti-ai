//! Multi-step turn loop (claw `run_turn` / opencode `runLoop` analogue).
//!
//! Drives `ConversationRuntime::run_turn` → tool dispatch → next turn
//! until the model emits `finish_reason = end_turn` (or the runtime hits
//! `max_steps` / cancellation / context-window error). Tool calls are
//! collected from the latest assistant message, dispatched via the
//! `ToolRegistry`, and appended back as `tool` role messages.
//!
//! Phase 4C scope: the loop itself + tool-result message append. Wiring
//! to the HTTP route + SSE permission events lands in Phase 5.

use std::sync::Arc;

use chrono::Utc;
use tokio_util::sync::CancellationToken;

use crate::adapters::artifact_store::ArtifactStore;
use crate::adapters::event_sink::{EventSink, Persistence};
use crate::adapters::filesystem::Filesystem;
use crate::adapters::memory_store::MemoryStore;
use crate::adapters::model_provider::FinishReason;
use crate::adapters::permission_manager::PermissionManager;
use crate::adapters::tool_executor::ToolCtx;
use crate::error::CoreError;
use crate::projection::LlmMessage;
use crate::tools::{
    ReadHistory, ToolDispatchResult, ToolInvocation, ToolRegistry, dispatch_batch,
};
use crate::types::agent::AgentId;
use crate::types::event::AgentEvent;
use crate::types::message::{Message, MessageId, Role};
use crate::types::part::{Part, PartId};
use crate::types::permission::{PermissionCtx, PermissionMode};
use crate::types::session::SessionId;

use super::ConversationRuntime;
use super::TurnInput;

/// Per-loop tunables. Caller owns the workspace + read-history handle so
/// the same `ReadHistory` carries through every step of the same session.
#[derive(Clone)]
pub struct LoopContext {
    pub agent_id: AgentId,
    pub fs: Arc<dyn Filesystem>,
    pub permission: Arc<dyn PermissionManager>,
    pub events: Arc<dyn EventSink>,
    pub artifacts: Arc<dyn ArtifactStore>,
    pub registry: Arc<ToolRegistry>,
    pub read_history: ReadHistory,
    pub mode: PermissionMode,
    pub max_steps: usize,
}

/// Outcome of a `run_loop` invocation.
#[derive(Debug)]
pub struct LoopOutcome {
    pub steps: usize,
    pub finish_reason: FinishReason,
    pub final_assistant_message_id: MessageId,
}

impl ConversationRuntime {
    /// Drive `run_turn` repeatedly until the model emits `end_turn` (or
    /// we hit `max_steps`). Each tool-use turn dispatches the requested
    /// tools and appends their results as a fresh `tool`-role message
    /// before the next LLM call.
    pub async fn run_loop(
        &self,
        memory: &Arc<dyn MemoryStore>,
        loop_ctx: LoopContext,
        mut input: TurnInput,
        cancel: CancellationToken,
    ) -> Result<LoopOutcome, CoreError> {
        let session_id = input.session_id;
        let mut last_assistant_id: Option<MessageId> = None;
        for step in 1..=loop_ctx.max_steps {
            let outcome = self.run_turn(input.clone(), cancel.clone()).await?;
            last_assistant_id = Some(outcome.assistant_message_id);
            if !matches!(outcome.finish_reason, FinishReason::ToolUse) {
                return Ok(LoopOutcome {
                    steps: step,
                    finish_reason: outcome.finish_reason,
                    final_assistant_message_id: outcome.assistant_message_id,
                });
            }

            // Collect tool_calls from the assistant message just produced.
            let invocations =
                collect_tool_calls(memory, session_id, outcome.assistant_message_id).await?;
            if invocations.is_empty() {
                return Ok(LoopOutcome {
                    steps: step,
                    finish_reason: FinishReason::EndTurn,
                    final_assistant_message_id: outcome.assistant_message_id,
                });
            }

            // Dispatch.
            let perm_ctx = PermissionCtx {
                session_id,
                mode: loop_ctx.mode,
            };
            let lc = loop_ctx.clone();
            let assistant_msg_id = outcome.assistant_message_id;
            let cancel_for_ctx = cancel.clone();
            let ctx_for = move |inv: &ToolInvocation| ToolCtx {
                session_id,
                agent_id: lc.agent_id,
                message_id: assistant_msg_id,
                call_id: inv.call_id.clone(),
                fs: Arc::clone(&lc.fs),
                mode: lc.mode,
                permission: Arc::clone(&lc.permission),
                events: Arc::clone(&lc.events),
                artifacts: Arc::clone(&lc.artifacts),
                read_history: lc.read_history.clone(),
                cancel: cancel_for_ctx.clone(),
            };
            let results = dispatch_batch(
                &loop_ctx.registry,
                &loop_ctx.permission,
                ctx_for,
                perm_ctx,
                invocations,
            )
            .await;

            // Append a tool-role message holding all results.
            let tool_msg_id = append_tool_message(memory, &loop_ctx.events, session_id, &results)
                .await?;
            // Project all messages so far into the next LLM input.
            input.messages = project_session_messages(memory, session_id).await?;
            let _ = tool_msg_id;
        }
        Ok(LoopOutcome {
            steps: loop_ctx.max_steps,
            finish_reason: FinishReason::MaxTokens,
            final_assistant_message_id: last_assistant_id.unwrap_or_default(),
        })
    }
}

async fn collect_tool_calls(
    memory: &Arc<dyn MemoryStore>,
    session_id: SessionId,
    message_id: MessageId,
) -> Result<Vec<ToolInvocation>, CoreError> {
    let parts = memory.list_parts(session_id, message_id).await?;
    let mut out = Vec::new();
    for p in parts {
        if let Part::ToolCall {
            call_id,
            name,
            args,
            ..
        } = p
        {
            out.push(ToolInvocation {
                call_id,
                name,
                args,
            });
        }
    }
    Ok(out)
}

async fn append_tool_message(
    memory: &Arc<dyn MemoryStore>,
    events: &Arc<dyn EventSink>,
    session_id: SessionId,
    results: &[ToolDispatchResult],
) -> Result<MessageId, CoreError> {
    let msg = Message {
        id: MessageId::new(),
        session_id,
        role: Role::Tool,
        created_at: Utc::now(),
    };
    let mid = memory.append_message(session_id, msg).await?;
    events
        .publish(
            AgentEvent::MessageCreated {
                session_id,
                message_id: mid,
                at: Utc::now(),
            },
            Persistence::Durable,
        )
        .await?;

    for r in results {
        let part_id = PartId::new();
        let part = match &r.outcome {
            Ok(value) => Part::ToolResult {
                id: part_id,
                call_id: r.call_id.clone(),
                ok: true,
                text: Some(value.to_string()),
                error: None,
            },
            Err(err) => Part::ToolResult {
                id: part_id,
                call_id: r.call_id.clone(),
                ok: false,
                text: None,
                error: Some(err.to_string()),
            },
        };
        memory.append_part(mid, part).await?;
        events
            .publish(
                AgentEvent::PartCreated {
                    session_id,
                    message_id: mid,
                    part_id,
                    at: Utc::now(),
                },
                Persistence::Durable,
            )
            .await?;
    }
    Ok(mid)
}

async fn project_session_messages(
    memory: &Arc<dyn MemoryStore>,
    session_id: SessionId,
) -> Result<Vec<LlmMessage>, CoreError> {
    use std::collections::HashMap;

    use crate::projection::{ProjectionCaps, project_for_llm};
    let messages = memory.list_messages(session_id).await?;
    let mut parts_by_msg: HashMap<MessageId, Vec<Part>> = HashMap::with_capacity(messages.len());
    for m in &messages {
        let parts = memory.list_parts(session_id, m.id).await?;
        parts_by_msg.insert(m.id, parts);
    }
    Ok(project_for_llm(&messages, &parts_by_msg, ProjectionCaps::default()))
}
