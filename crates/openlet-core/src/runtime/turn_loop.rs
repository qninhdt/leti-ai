//! Multi-step turn loop.
//!
//! Drives `ConversationRuntime::run_turn` → tool dispatch → next turn
//! until the model emits `finish_reason = end_turn` (or the runtime hits
//! `max_steps` / cancellation / context-window error). Tool calls are
//! collected from the latest assistant message, dispatched via the
//! `ToolRegistry`, and appended back as `tool` role messages.
//!
//! The loop itself appends tool-result messages and drives turns.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::adapters::artifact_store::ArtifactStore;
use crate::adapters::event_sink::{EventSink, Persistence};
use crate::adapters::filesystem::Filesystem;
use crate::adapters::memory_store::MemoryStore;
use crate::adapters::model_provider::FinishReason;
use crate::adapters::permission_manager::PermissionManager;
use crate::adapters::tool_executor::ToolCtx;
use crate::dispatch::{DispatchOutcome, HookChains, dispatch, publish_fault_if_any};
use crate::error::CoreError;
use crate::hooks::io::{AfterTurnCtx, BeforeTurnCtx, OnStepFinishCtx};
use crate::runtime::agent_allowlist::{
    merge_with_denied, partition_by_allowlist, resolve_allowlist,
};
use crate::runtime::doom_guard::{self, DoomVerdict, TurnSummary as DoomTurnSummary};
use crate::runtime::question_registry::QuestionRegistry;
use crate::tools::{ReadHistory, ToolInvocation, ToolRegistry, dispatch_batch};
use crate::types::agent::AgentId;
use crate::types::event::AgentEvent;
use crate::types::message::MessageId;
use crate::types::permission::{PermissionCtx, PermissionMode};

use super::ConversationRuntime;
use super::TurnInput;
use super::turn_loop_helpers::{
    append_tool_message, build_doom_summary, collect_tool_calls, project_session_messages,
};

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
    /// In-process subagent task registry. Threaded through `ToolCtx`
    /// so `subagent_task` / `task_status` find their bookkeeping.
    pub task_registry: Arc<crate::runtime::subagent::TaskRegistry>,
    /// Resolves the session's current agent slug to an
    /// `AgentDefinition` at every tool dispatch (NOT once per turn).
    /// Wired through so plan-mode swaps the active profile
    /// MID-TURN and the next dispatch sees the new allowlist
    /// — no race window where a write tool sneaks past the gate
    /// because the loop snapshotted "general" before EnterPlanMode
    /// flipped the slug. Same handle the runtime uses to compact +
    /// spawn nested subagents.
    pub agent_registry: Arc<crate::agent::AgentRegistry>,
}

/// Outcome of a `run_loop` invocation.
#[derive(Debug)]
pub struct LoopOutcome {
    pub steps: usize,
    pub finish_reason: FinishReason,
    /// Id of the last assistant message the loop produced, or `None` when
    /// no model turn ran (e.g. a `before_turn` hook halted turn 0, or
    /// max_steps was 0). This was previously a non-optional
    /// `MessageId` defaulting to the nil UUID, which consumers (e.g.
    /// `subagent_spawner`) would feed into `list_parts` and silently match
    /// zero rows. `Option` forces callers to handle the no-message case.
    pub final_assistant_message_id: Option<MessageId>,
}

/// Build a `Halted` loop outcome from the current step counter and the
/// last real assistant message id. `final_assistant_message_id` is `None`
/// when no model turn has run yet (e.g. a `before_turn` hook halts turn 0)
/// rather than a nil-UUID sentinel.
fn halted_outcome(steps: usize, last_assistant_id: Option<MessageId>) -> LoopOutcome {
    LoopOutcome {
        steps,
        finish_reason: FinishReason::Halted,
        final_assistant_message_id: last_assistant_id,
    }
}

use super::turn_loop_compaction::CompactionFlow;

impl ConversationRuntime {
    /// Drive `run_turn` repeatedly until the model emits `end_turn` (or
    /// we hit `max_steps`). Each tool-use turn dispatches the requested
    /// tools and appends their results as a fresh `tool`-role message
    /// before the next LLM call.
    ///
    /// Wrapped in a coarse `turn` span carrying the correlation ids
    /// (`session_id` + a fresh `turn_id`) so every log line emitted
    /// during the turn — across dispatch, provider, and compaction — is
    /// attributable to one turn end-to-end. The span wraps the whole
    /// loop, NOT per-token work on the stream hot path.
    #[tracing::instrument(
        skip_all,
        fields(session_id = %input.session_id, turn_id = tracing::field::Empty)
    )]
    pub async fn run_loop(
        &self,
        memory: &Arc<dyn MemoryStore>,
        loop_ctx: LoopContext,
        mut input: TurnInput,
        cancel: CancellationToken,
    ) -> Result<LoopOutcome, CoreError> {
        use crate::runtime::compaction::{CompactDecision, should_compact};
        let session_id = input.session_id;
        // Per-invocation correlation id; recorded into the turn span so it
        // flows into the JSON logs alongside `session_id`.
        let turn_id = uuid::Uuid::new_v4();
        tracing::Span::current().record("turn_id", tracing::field::display(&turn_id));
        let mut last_assistant_id: Option<MessageId> = None;
        let mut last_actual_tokens: Option<usize> = None;
        // Count of projected messages sent on the turn that produced
        // `last_actual_tokens`. Messages beyond this in `input.messages`
        // are the "unsent tail" (tool results + the assistant turn just
        // produced) not yet reflected in the provider's prompt-token count.
        let mut sent_message_count: usize = 0;
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
                    // Estimate tokens for messages appended since the last
                    // provider-reported prompt size (tool results from the
                    // turn just completed). Added to `last_actual_tokens`
                    // so a large tool result triggers compaction this cycle
                    // rather than overflowing the window next turn.
                    let unsent_tail_tokens = input
                        .messages
                        .get(sent_message_count..)
                        .map(crate::runtime::token_estimate::estimate_conversation_tokens)
                        .unwrap_or(0);
                    if let CompactDecision::Run { keep } = should_compact(
                        &input.messages,
                        agent,
                        last_actual_tokens,
                        unsent_tail_tokens,
                    ) {
                        compacted_this_loop = true;
                        match self
                            .run_compaction(
                                memory,
                                &loop_ctx,
                                agent,
                                keep,
                                &mut input,
                                &mut last_actual_tokens,
                                cancel.clone(),
                            )
                            .await?
                        {
                            CompactionFlow::Continue => continue,
                            CompactionFlow::Halt => {
                                return Ok(halted_outcome(model_steps, last_assistant_id));
                            }
                        }
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
                        return Ok(halted_outcome(model_steps, last_assistant_id));
                    }
                    DispatchOutcome::Denied {
                        reason,
                        feedback,
                        plugin_fault,
                    } => {
                        crate::dispatch::publish_denied_warn(
                            &loop_ctx.events,
                            Some(session_id),
                            "before_turn",
                            &reason,
                            &feedback,
                            plugin_fault.as_ref(),
                        )
                        .await;
                        return Ok(halted_outcome(model_steps, last_assistant_id));
                    }
                }
            }
            let outcome = self.run_turn(input.clone(), cancel.clone()).await?;
            model_steps += 1;
            last_assistant_id = Some(outcome.assistant_message_id);
            if let Some(u) = outcome.usage.as_ref() {
                last_actual_tokens = Some(u.input_tokens as usize);
                // Anchor the unsent-tail boundary: these are the messages
                // whose tokens `provider_actual` now accounts for. Tool
                // results appended after this turn fall beyond the boundary
                // and are estimated separately by the next compaction check.
                sent_message_count = input.messages.len();
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
                    final_assistant_message_id: Some(outcome.assistant_message_id),
                });
            }

            // Collect tool_calls from the assistant message just produced.
            let invocations =
                collect_tool_calls(memory, session_id, outcome.assistant_message_id).await?;
            if invocations.is_empty() {
                return Ok(LoopOutcome {
                    steps: model_steps,
                    finish_reason: FinishReason::EndTurn,
                    final_assistant_message_id: Some(outcome.assistant_message_id),
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
                task_registry: Arc::clone(&lc.task_registry),
                agent_registry: Arc::clone(&lc.agent_registry),
            };
            // Snapshot the active agent slug RIGHT BEFORE dispatch
            // (not at turn start). A previous tool in the same loop may
            // have swapped the agent (EnterPlanMode), so the next batch
            // must see the new allowlist.
            let allowlist_outcome =
                resolve_allowlist(memory, session_id, Some(&loop_ctx.agent_registry)).await;
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
                    final_assistant_message_id: Some(outcome.assistant_message_id),
                });
            }

            // Append a tool-role message holding all results.
            append_tool_message(memory, &loop_ctx.events, session_id, &results).await?;
            // Project all messages so far into the next LLM input.
            input.messages = project_session_messages(memory, session_id).await?;
        }
        Ok(LoopOutcome {
            steps: loop_ctx.max_steps,
            finish_reason: FinishReason::MaxSteps,
            final_assistant_message_id: last_assistant_id,
        })
    }
}

#[cfg(test)]
mod halted_outcome_tests {
    //! `halted_outcome` must surface `final_assistant_message_id =
    //! None` when no model turn produced an assistant message (e.g. a
    //! `before_turn` hook halts turn 0). Previously it returned
    //! `MessageId::default()` (the nil UUID), which the subagent consumer
    //! fed into `list_parts`, silently matching zero rows and masking the
    //! "no message" case as a real-but-empty lookup.
    use super::*;

    #[test]
    fn halt_with_no_prior_turn_yields_none_not_nil_uuid() {
        let outcome = halted_outcome(0, None);
        assert_eq!(outcome.finish_reason, FinishReason::Halted);
        assert_eq!(outcome.steps, 0);
        assert!(
            outcome.final_assistant_message_id.is_none(),
            "halt before any model turn must be None, NOT a nil-UUID sentinel"
        );
    }

    #[test]
    fn halt_after_a_real_turn_preserves_the_message_id() {
        let mid = MessageId::new();
        let outcome = halted_outcome(3, Some(mid));
        assert_eq!(outcome.finish_reason, FinishReason::Halted);
        assert_eq!(outcome.steps, 3);
        assert_eq!(
            outcome.final_assistant_message_id,
            Some(mid),
            "halt after a model turn must carry that turn's assistant message id"
        );
    }
}
