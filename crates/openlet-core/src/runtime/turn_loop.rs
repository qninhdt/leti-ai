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
use crate::agent::AgentRegistry;
use crate::dispatch::{DispatchOutcome, HookChains, dispatch, publish_fault_if_any};
use crate::error::CoreError;
use crate::hooks::io::{
    AfterTurnCtx, BeforeTurnCtx, CompactionPhase, OnCompactionCtx, OnStepFinishCtx,
};
use crate::projection::LlmMessage;
use crate::runtime::agent_allowlist::{
    merge_with_denied, partition_by_allowlist, resolve_allowlist,
};
use crate::runtime::doom_guard::{self, DoomVerdict, ToolCallSig, TurnSummary as DoomTurnSummary};
use crate::runtime::question_registry::QuestionRegistry;
use crate::tools::{ReadHistory, ToolDispatchResult, ToolInvocation, ToolRegistry, dispatch_batch};
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
    /// Sorted plugin hook chains. `Arc::new(HookChains::new())` when no
    /// plugins register hooks — dispatch is O(1) skip on empty chains.
    pub hook_chains: Arc<HookChains>,
    /// Question rendezvous map. Forwarded into each tool's `ToolCtx` so
    /// `ask_user` can register a oneshot before suspending on the receiver.
    pub questions: Arc<QuestionRegistry>,
    /// Memory store handle. Forwarded into each tool's `ToolCtx` so
    /// session-aware tools (e.g. `ask_user` capability gate) can read
    /// session metadata without an extra adapter wired through every
    /// caller.
    pub memory: Arc<dyn MemoryStore>,
    /// Resolves the session's current agent slug to an
    /// `AgentDefinition` at every tool dispatch (NOT once per turn).
    /// `None` ⇒ allowlist enforcement is disabled (legacy callers,
    /// tests). Wired through so plan-mode swaps the active profile
    /// MID-TURN and the next dispatch sees the new allowlist
    /// — no race window where a write tool sneaks past the gate
    /// because the loop snapshotted "general" before EnterPlanMode
    /// flipped the slug.
    pub agent_registry: Option<Arc<AgentRegistry>>,
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
            CompactDecision, append_compaction_part, append_synthetic_request, should_compact,
            superseded_messages,
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
        let mut doom_history: Vec<DoomTurnSummary> = Vec::new();
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
                        let pre_msg_count = input.messages.len();
                        // OnCompaction Before — Stop halts compaction; the
                        // outer loop continues with the un-compacted projection.
                        // `autocontinue` is set to its default for completeness;
                        // the toggle is only honored on the After-phase dispatch.
                        let before_ctx = OnCompactionCtx {
                            session_id: Some(session_id),
                            phase: CompactionPhase::Before,
                            message_count: pre_msg_count,
                            autocontinue: true,
                        };
                        let before_outcome =
                            dispatch(&loop_ctx.hook_chains.on_compaction, before_ctx).await;
                        publish_fault_if_any(&loop_ctx.events, Some(session_id), &before_outcome)
                            .await;
                        if matches!(
                            before_outcome,
                            DispatchOutcome::Stopped(_) | DispatchOutcome::Denied { .. }
                        ) {
                            continue;
                        }
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
                        // Build a one-shot compaction projection and run a
                        // turn. The result text becomes Part::Compaction.
                        let mut compact_input = input.clone();
                        compact_input.messages =
                            crate::runtime::compaction::build_compaction_projection(
                                &input.messages,
                                keep,
                            );
                        compact_input.tools = Vec::new();
                        // If the compaction turn fails or is cancelled, the
                        // synthetic "Summarize the conversation above" message
                        // remains in storage as a real user turn. Roll it back
                        // by superseding it in a no-op compaction part rather
                        // than leaving it visible to subsequent projections.
                        let outcome = match self.run_turn(compact_input, cancel.clone()).await {
                            Ok(o) => o,
                            Err(e) => {
                                let _ = append_compaction_part(
                                    memory,
                                    &loop_ctx.events,
                                    session_id,
                                    String::new(),
                                    vec![synth_id],
                                    0,
                                )
                                .await;
                                return Err(e);
                            }
                        };
                        // Drain the freshly produced assistant text into a Compaction part.
                        let summary = collect_assistant_text(
                            memory,
                            session_id,
                            outcome.assistant_message_id,
                        )
                        .await?;
                        // Refuse to persist an empty summary — that would
                        // supersede older messages with a blank string and
                        // silently lose history. Roll back the synthetic
                        // request and the empty assistant turn, then bubble
                        // the failure up so the caller can retry.
                        if summary.trim().is_empty() {
                            let _ = append_compaction_part(
                                memory,
                                &loop_ctx.events,
                                session_id,
                                String::new(),
                                vec![synth_id, outcome.assistant_message_id],
                                0,
                            )
                            .await;
                            return Err(CoreError::ContextOverflowAfterCompaction);
                        }
                        superseded.push(synth_id);
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
                        // OnCompaction After — observation; plugins emit
                        // metrics or post-process the new projection. Stop
                        // does not unwind the compaction (already durable).
                        // A plugin may also set `autocontinue = false` via
                        // Replace to pause the loop instead of driving the
                        // synthetic resume turn — see handling below.
                        let after_ctx = OnCompactionCtx {
                            session_id: Some(session_id),
                            phase: CompactionPhase::After,
                            message_count: input.messages.len(),
                            autocontinue: true,
                        };
                        let after_outcome =
                            dispatch(&loop_ctx.hook_chains.on_compaction, after_ctx).await;
                        publish_fault_if_any(&loop_ctx.events, Some(session_id), &after_outcome)
                            .await;
                        // Honor the autocontinue toggle: when a plugin
                        // returns Replace with autocontinue=false from the
                        // After phase, skip the synthetic resume turn,
                        // signal the pause via SessionStatus::Idle, and
                        // exit the loop. SessionStatus::Idle is used as a
                        // proxy until a dedicated Paused variant lands.
                        if let DispatchOutcome::Completed(ref ctx) = after_outcome {
                            if !ctx.autocontinue {
                                let _ = loop_ctx
                                    .events
                                    .publish(
                                        AgentEvent::SessionStatus {
                                            session_id,
                                            status: crate::types::session::SessionStatus::Idle,
                                            at: Utc::now(),
                                        },
                                        Persistence::Durable,
                                    )
                                    .await;
                                return Ok(LoopOutcome {
                                    steps: model_steps,
                                    finish_reason: FinishReason::Halted,
                                    final_assistant_message_id: last_assistant_id
                                        .unwrap_or_default(),
                                });
                            }
                        }
                        // Post-compaction overflow check (amendment §P).
                        let post = should_compact(&input.messages, agent, None);
                        if matches!(post, CompactDecision::Run { .. }) {
                            return Err(CoreError::ContextOverflowAfterCompaction);
                        }
                        continue;
                    }
                }
            }
            // BeforeTurn hook chain — Stop halts the loop with finish_reason=Halted;
            // Deny short-circuits via a synthetic tool-result on the next turn.
            // O(1) skip when empty.
            if !loop_ctx.hook_chains.before_turn.is_empty() {
                let before_ctx = BeforeTurnCtx {
                    session_id: Some(session_id),
                    turn_index: model_steps as u32,
                    message_count: input.messages.len(),
                };
                match dispatch(&loop_ctx.hook_chains.before_turn, before_ctx).await {
                    DispatchOutcome::Completed(_) => {}
                    DispatchOutcome::Stopped(_) => {
                        return Ok(LoopOutcome {
                            steps: model_steps,
                            finish_reason: FinishReason::Halted,
                            final_assistant_message_id: last_assistant_id.unwrap_or_default(),
                        });
                    }
                    DispatchOutcome::Denied {
                        reason,
                        feedback,
                        plugin_fault,
                    } => {
                        if let Some(fault) = plugin_fault.as_ref() {
                            let _ = loop_ctx
                                .events
                                .publish(
                                    crate::dispatch::plugin_error_event(Some(session_id), fault),
                                    crate::adapters::event_sink::Persistence::Durable,
                                )
                                .await;
                        }
                        tracing::warn!(
                            reason = %reason,
                            feedback = ?feedback,
                            "before_turn denied; halting loop",
                        );
                        return Ok(LoopOutcome {
                            steps: model_steps,
                            finish_reason: FinishReason::Halted,
                            final_assistant_message_id: last_assistant_id.unwrap_or_default(),
                        });
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

            // AfterTurn hook chain — observation only; does not change loop control flow.
            // O(1) skip when empty.
            if !loop_ctx.hook_chains.after_turn.is_empty() {
                let after_ctx = AfterTurnCtx {
                    session_id: Some(session_id),
                    turn_index: model_steps as u32,
                    finish_reason: Some(outcome.finish_reason),
                    usage: outcome.usage.clone(),
                    cost_usd: outcome.cost_usd,
                };
                let after_outcome = dispatch(&loop_ctx.hook_chains.after_turn, after_ctx).await;
                publish_fault_if_any(&loop_ctx.events, Some(session_id), &after_outcome).await;
            }

            // OnStepFinish — fires once per loop iteration so audit/quota
            // plugins can roll up usage even when the model continues
            // with a tool turn (AfterTurn fires too, but observers may
            // care about the per-step granularity). O(1) skip when empty.
            if !loop_ctx.hook_chains.on_step_finish.is_empty() {
                let step_ctx = OnStepFinishCtx {
                    session_id: Some(session_id),
                    step_index: model_steps as u32,
                    finish_reason: Some(outcome.finish_reason),
                    usage: outcome.usage.clone(),
                };
                let step_outcome = dispatch(&loop_ctx.hook_chains.on_step_finish, step_ctx).await;
                publish_fault_if_any(&loop_ctx.events, Some(session_id), &step_outcome).await;
            }

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
                questions: Arc::clone(&lc.questions),
                memory: Arc::clone(&lc.memory),
            };
            // F2.5 — snapshot the active agent slug RIGHT BEFORE dispatch
            // (not at turn start). A previous tool in the same loop may
            // have swapped the agent (EnterPlanMode), so the next batch
            // must see the new allowlist.
            let allowlist_outcome =
                resolve_allowlist(memory, session_id, loop_ctx.agent_registry.as_ref()).await;
            let (allowed, denied) =
                partition_by_allowlist(&invocations, allowlist_outcome.as_ref());
            let dispatched_results = if allowed.is_empty() {
                Vec::new()
            } else {
                dispatch_batch(
                    &loop_ctx.registry,
                    &loop_ctx.permission,
                    &loop_ctx.hook_chains,
                    &loop_ctx.events,
                    session_id,
                    ctx_for,
                    perm_ctx,
                    allowed,
                )
                .await
            };
            let results = merge_with_denied(&invocations, dispatched_results, denied);

            // Doom-guard check. Abort the loop if the model has spent the
            // last `DEFAULT_THRESHOLD` turns issuing the same (or strictly
            // narrowing) tool-call set without producing text or successful
            // tool results. `doom_guard::check` is pure; we feed it a rolling
            // history capped at `threshold + 1` to keep the slice cheap.
            let summary = build_doom_summary(
                memory,
                session_id,
                outcome.assistant_message_id,
                &invocations,
                &results,
            )
            .await?;
            doom_history.push(summary);
            if doom_history.len() > doom_guard::DEFAULT_THRESHOLD + 1 {
                let drop = doom_history.len() - (doom_guard::DEFAULT_THRESHOLD + 1);
                doom_history.drain(0..drop);
            }
            if let DoomVerdict::Abort { message } =
                doom_guard::check(&doom_history, doom_guard::DEFAULT_THRESHOLD)
            {
                tracing::warn!(session_id = %session_id, "doom-loop detected; halting");
                let _ = loop_ctx
                    .events
                    .publish(
                        AgentEvent::Error {
                            session_id: Some(session_id),
                            code: "doom_loop".into(),
                            message: message.clone(),
                        },
                        Persistence::Durable,
                    )
                    .await;
                return Ok(LoopOutcome {
                    steps: model_steps,
                    finish_reason: FinishReason::Halted,
                    final_assistant_message_id: outcome.assistant_message_id,
                });
            }

            // Append a tool-role message holding all results.
            let tool_msg_id =
                append_tool_message(memory, &loop_ctx.events, session_id, &results).await?;
            // Project all messages so far into the next LLM input.
            input.messages = project_session_messages(memory, session_id).await?;
            let _ = tool_msg_id;
        }
        Ok(LoopOutcome {
            steps: loop_ctx.max_steps,
            finish_reason: FinishReason::MaxSteps,
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
    Ok(project_for_llm(
        &messages,
        &parts_by_msg,
        ProjectionCaps::default(),
    ))
}

/// Build a `TurnSummary` for the doom-guard from the assistant message just
/// produced + the freshly-dispatched tool results. `had_text_output` reflects
/// any non-empty `Part::Text` body; `had_successful_writes` is any tool result
/// with `ok=true` from a tool the registry marks `parallel_safe=false` (write
/// tools serialize). Tool-call signatures use `ToolCallSig::new` over the
/// invocation's parsed args.
async fn build_doom_summary(
    memory: &Arc<dyn MemoryStore>,
    session_id: SessionId,
    assistant_message_id: MessageId,
    invocations: &[ToolInvocation],
    results: &[ToolDispatchResult],
) -> Result<DoomTurnSummary, CoreError> {
    let parts = memory.list_parts(session_id, assistant_message_id).await?;
    let had_text_output = parts
        .iter()
        .any(|p| matches!(p, Part::Text { text, .. } if !text.trim().is_empty()));
    let had_successful_writes = results
        .iter()
        .any(|r| r.outcome.is_ok() && !is_read_only_tool(&r.name));
    let mut tool_calls = std::collections::BTreeSet::new();
    for inv in invocations {
        tool_calls.insert(ToolCallSig::new(inv.name.clone(), &inv.args));
    }
    Ok(DoomTurnSummary {
        had_text_output,
        had_successful_writes,
        tool_calls,
    })
}

/// Names of read-only tools — used by the doom-guard to discriminate
/// "writes succeeded" from "reads succeeded." Read-only successes don't
/// reset the loop counter: an agent looping on `read` of the same path
/// is still looping.
fn is_read_only_tool(name: &str) -> bool {
    matches!(name, "read" | "list" | "glob" | "grep")
}
