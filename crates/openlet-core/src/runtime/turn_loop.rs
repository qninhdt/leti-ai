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
    /// Optional agent definition. When provided, compaction triggers
    /// at the top of each loop iteration once projected tokens cross
    /// `agent.context_window * agent.compaction_threshold`.
    pub agent: Option<Arc<crate::agent::AgentDefinition>>,
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
        use crate::runtime::compaction::{
            CompactDecision, append_compaction_part, append_synthetic_request,
            should_compact, superseded_messages,
        };
        let session_id = input.session_id;
        let mut last_assistant_id: Option<MessageId> = None;
        let mut last_actual_tokens: Option<usize> = None;
        let mut compacted_this_loop = false;
        // Compaction iterations don't count against the model-step budget;
        // they're not the model doing user-visible work, just keeping the
        // window healthy. Track the budget explicitly so the `continue;`
        // after a compaction doesn't burn a step.
        let mut model_steps: usize = 0;
        loop {
            if model_steps >= loop_ctx.max_steps {
                break;
            }
            // Compaction check at top of each iteration. Skipped during a
            // compaction-induced turn to avoid recursion.
            if let Some(agent) = loop_ctx.agent.as_ref() {
                if !compacted_this_loop {
                    let decision = should_compact(&input.messages, agent, last_actual_tokens);
                    if let CompactDecision::Run { keep } = decision {
                        compacted_this_loop = true;
                        // Determine which existing messages will be superseded.
                        let messages = memory.list_messages(session_id).await?;
                        let mut superseded = superseded_messages(&messages, keep);
                        let original_tokens =
                            crate::runtime::token_estimate::estimate_conversation_tokens(
                                &input.messages,
                            ) as u32;
                        // Append synthetic user message asking for summary.
                        // The synthetic id is added to `superseded` so the
                        // projection substitutes the summary in its place
                        // — otherwise the next turn would see the literal
                        // "Summarize the conversation history above" prompt
                        // it never issued.
                        let synth_id =
                            append_synthetic_request(memory, &loop_ctx.events, session_id).await?;
                        superseded.push(synth_id);
                        // Build a one-shot compaction projection and run a
                        // turn. The result text becomes Part::Compaction.
                        let mut compact_input = input.clone();
                        compact_input.messages = crate::runtime::compaction::build_compaction_projection(
                            &input.messages,
                            keep,
                        );
                        compact_input.tools = Vec::new();
                        let outcome = self.run_turn(compact_input, cancel.clone()).await?;
                        // Drain the freshly produced assistant text into a Compaction part.
                        let summary = collect_assistant_text(
                            memory,
                            session_id,
                            outcome.assistant_message_id,
                        )
                        .await?;
                        // The compaction-turn's assistant message holds the
                        // verbatim summary as Part::Text; substitute it via
                        // the Compaction part on subsequent projections so
                        // the model doesn't see the summary twice.
                        superseded.push(outcome.assistant_message_id);
                        if !superseded.is_empty() {
                            let _comp_id = append_compaction_part(
                                memory,
                                &loop_ctx.events,
                                session_id,
                                summary,
                                superseded,
                                original_tokens,
                            )
                            .await?;
                        }
                        // Re-project so the next turn sees the summary in
                        // place of the compacted messages.
                        input.messages = project_session_messages(memory, session_id).await?;
                        // Reset provider-actual anchor — last value referred
                        // to the pre-compaction prompt and is now stale.
                        last_actual_tokens = None;
                        // Post-compaction overflow check (amendment §P).
                        let post = should_compact(&input.messages, agent, None);
                        if matches!(post, CompactDecision::Run { .. }) {
                            return Err(CoreError::ContextOverflowAfterCompaction);
                        }
                        continue;
                    }
                }
            }
            let outcome = self.run_turn(input.clone(), cancel.clone()).await?;
            model_steps += 1;
            last_assistant_id = Some(outcome.assistant_message_id);
            if let Some(u) = outcome.usage.as_ref() {
                last_actual_tokens = Some(u.input_tokens as usize);
            }
            // Reset the per-loop compaction guard after a real model turn
            // — subsequent turns may need to compact again.
            compacted_this_loop = false;
            if !matches!(outcome.finish_reason, FinishReason::ToolUse) {
                return Ok(LoopOutcome {
                    steps: model_steps,
                    finish_reason: outcome.finish_reason,
                    final_assistant_message_id: outcome.assistant_message_id,
                });
            }

            // Collect tool_calls from the assistant message just produced.
            let invocations =
                collect_tool_calls(memory, session_id, outcome.assistant_message_id).await?;
            if invocations.is_empty() {
                return Ok(LoopOutcome {
                    steps: model_steps,
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

/// Concatenate every `Part::Text` body on a single message. Used after a
/// compaction turn to fold the assistant's reply into a Compaction part.
async fn collect_assistant_text(
    memory: &Arc<dyn MemoryStore>,
    session_id: SessionId,
    message_id: MessageId,
) -> Result<String, CoreError> {
    let parts = memory.list_parts(session_id, message_id).await?;
    let mut buf = String::new();
    for p in parts {
        if let Part::Text { text, .. } = p {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(&text);
        }
    }
    Ok(buf)
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
