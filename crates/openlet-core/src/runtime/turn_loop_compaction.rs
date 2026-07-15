//! Compaction step for the multi-step turn loop.
//!
//! Extracted from `turn_loop.rs` to keep the loop body focused on
//! control flow. Owns the `OnCompaction` Before/After hook dispatch, the
//! one-shot summarization turn, summary persistence as `Part::Compaction`,
//! and the post-compaction overflow check.

use std::sync::Arc;

use chrono::Utc;
use tokio_util::sync::CancellationToken;

use crate::adapters::event_sink::Persistence;
use crate::adapters::memory_store::MemoryStore;
use crate::dispatch::{DispatchOutcome, dispatch, publish_fault_if_any};
use crate::error::CoreError;
use crate::hooks::io::{CompactionPhase, OnCompactionCtx};
use crate::types::event::AgentEvent;
use crate::types::message::MessageId;
use crate::types::part::{CompactionAttemptState, Part, PartId};
use crate::types::session::SessionId;

use super::ConversationRuntime;
use super::TurnInput;
use super::turn_loop::LoopContext;
use super::turn_loop_helpers::collect_assistant_text;

/// Signal returned by `run_compaction` telling `run_loop` how to proceed
/// after a compaction step. Hard failures propagate as `Err` instead.
pub(super) enum CompactionFlow {
    /// Restart the loop iteration (compaction ran, or the `Before` hook
    /// halted it without compacting). Does not burn a model step.
    Continue,
    /// A plugin paused the loop via `autocontinue = false` on the After
    /// phase — exit cleanly without driving a synthetic resume turn.
    Halt,
}

impl ConversationRuntime {
    /// Manually trigger one compaction step, bypassing the token-threshold
    /// gate `run_loop` applies. Backs the on-demand `/compact` command so a
    /// user can summarize before context pressure forces it. Compacts
    /// everything older than the most recent `PRESERVE_RECENT` messages,
    /// then returns — it does NOT drive a follow-up model turn (that's the
    /// loop's job when compaction fires mid-turn).
    ///
    /// Returns `Ok(false)` when there's nothing to compact (no agent, or the
    /// conversation is at/under the preserved-recent floor).
    pub async fn compact_session(
        &self,
        memory: &Arc<dyn MemoryStore>,
        loop_ctx: &LoopContext,
        mut input: TurnInput,
        cancel: CancellationToken,
    ) -> Result<bool, CoreError> {
        use crate::projection::LlmRole;
        use crate::runtime::compaction::PRESERVE_RECENT;

        let Some(agent) = loop_ctx.agent.as_ref() else {
            return Ok(false);
        };
        // Count non-system messages; nothing to compact if we're at or under
        // the preserved-recent floor (compaction would supersede nothing).
        let body_count = input
            .messages
            .iter()
            .filter(|m| !matches!(m.role, LlmRole::System))
            .count();
        if body_count <= PRESERVE_RECENT {
            return Ok(false);
        }
        let keep = PRESERVE_RECENT.min(body_count);
        let mut last_actual_tokens: Option<usize> = None;
        self.run_compaction(
            memory,
            loop_ctx,
            agent.as_ref(),
            keep,
            &mut input,
            &mut last_actual_tokens,
            cancel,
        )
        .await?;
        Ok(true)
    }

    /// Run one compaction step: dispatch the `OnCompaction` Before/After
    /// hooks, drive a one-shot summarization turn, persist the summary as
    /// `Part::Compaction`, and re-project.
    ///
    /// Returns [`CompactionFlow`] telling the loop whether to `continue`
    /// or halt; hard failures (turn error, empty/overflowing summary)
    /// propagate as `Err`.
    #[allow(clippy::too_many_arguments)]
    #[tracing::instrument(
        skip_all,
        fields(session_id = %input.session_id, keep = keep)
    )]
    pub(super) async fn run_compaction(
        &self,
        memory: &Arc<dyn MemoryStore>,
        loop_ctx: &LoopContext,
        agent: &crate::agent::AgentDefinition,
        keep: usize,
        input: &mut TurnInput,
        last_actual_tokens: &mut Option<usize>,
        cancel: CancellationToken,
    ) -> Result<CompactionFlow, CoreError> {
        use crate::runtime::compaction::{
            CompactDecision, append_compaction_part, append_synthetic_request,
            build_compaction_projection, should_compact, superseded_messages,
        };
        use crate::runtime::token_estimate::estimate_conversation_tokens;

        let session_id = input.session_id;
        let pre_msg_count = input.messages.len();
        // OnCompaction Before — Stop halts compaction; the outer loop
        // continues with the un-compacted projection. `autocontinue` is
        // set to its default for completeness; the toggle is only honored
        // on the After-phase dispatch.
        let before_ctx = OnCompactionCtx {
            session_id: Some(session_id),
            phase: CompactionPhase::Before,
            message_count: pre_msg_count,
            autocontinue: true,
        };
        let before_outcome =
            dispatch(&loop_ctx.handles.hook_chains.on_compaction, before_ctx).await;
        publish_fault_if_any(&loop_ctx.handles.events, Some(session_id), &before_outcome).await;
        if matches!(
            before_outcome,
            DispatchOutcome::Stopped(_) | DispatchOutcome::Denied { .. }
        ) {
            return Ok(CompactionFlow::Continue);
        }
        // Determine which existing messages will be superseded.
        let messages = memory.list_messages(session_id).await?;
        let superseded = superseded_messages(&messages, keep);
        let original_tokens = estimate_conversation_tokens(&input.messages) as u32;
        // Persist a typed request marker. Its message id is added to
        // `superseded` so the projection substitutes the
        // summary in its place — otherwise the next turn would see the
        // literal "Summarize the conversation history above" prompt it
        // never issued.
        let (request_message_id, request_part_id) =
            append_synthetic_request(memory, &loop_ctx.handles.events, session_id).await?;
        // Build a one-shot compaction projection and run a turn. The
        // result text becomes Part::Compaction.
        let mut compact_input = input.clone();
        // Reload after persisting the typed marker so compaction uses the
        // same request-preparation boundary as every normal provider call.
        let fresh = crate::runtime::request_prep::prepare_compaction_session_messages(
            &loop_ctx.handles,
            session_id,
            loop_ctx.projection_caps,
            &crate::runtime::compaction::COMPACTION_REQUEST,
        )
        .await?;
        compact_input.messages = build_compaction_projection(&fresh, keep);
        compact_input.tools = Vec::new();
        // A failed or cancelled attempt is marked failed, so the typed
        // request remains hidden from both the timeline and later projections.
        let outcome = match self.run_turn(compact_input, cancel).await {
            Ok(o) => o,
            Err(e) => {
                let summary_message_id =
                    latest_attempt_assistant(memory, session_id, request_message_id).await?;
                set_compaction_request_state(
                    memory,
                    &loop_ctx.handles.events,
                    session_id,
                    request_message_id,
                    request_part_id,
                    CompactionAttemptState::Failed,
                    summary_message_id,
                )
                .await?;
                return Err(e);
            }
        };
        // Drain the freshly produced assistant text into a Compaction part.
        let summary =
            collect_assistant_text(memory, session_id, outcome.assistant_message_id).await?;
        // Refuse to persist an empty summary — that would supersede older
        // messages with a blank string and silently lose history. Roll
        // back the synthetic request and the empty assistant turn, then
        // bubble the failure up so the caller can retry.
        if summary.trim().is_empty() {
            set_compaction_request_state(
                memory,
                &loop_ctx.handles.events,
                session_id,
                request_message_id,
                request_part_id,
                CompactionAttemptState::Failed,
                Some(outcome.assistant_message_id.0.to_string()),
            )
            .await?;
            return Err(CoreError::ContextOverflowAfterCompaction);
        }
        let _comp_id = append_compaction_part(
            memory,
            &loop_ctx.handles.events,
            session_id,
            outcome.assistant_message_id,
            summary,
            superseded,
            original_tokens,
        )
        .await?;
        set_compaction_request_state(
            memory,
            &loop_ctx.handles.events,
            session_id,
            request_message_id,
            request_part_id,
            CompactionAttemptState::Committed,
            Some(outcome.assistant_message_id.0.to_string()),
        )
        .await?;
        // Compaction committed durably — count it here, after the part is
        // persisted, so a rolled-back/failed attempt above isn't counted.
        metrics::counter!("openlet_compactions_total").increment(1);
        // Re-project so the next turn sees the summary in place of the
        // compacted messages.
        input.messages = crate::runtime::request_prep::prepare_session_messages(
            &loop_ctx.handles,
            session_id,
            loop_ctx.projection_caps,
            crate::runtime::request_prep::ReminderRequestContext {
                turn_index: 0,
                max_turns: loop_ctx.max_steps,
                actual_input_tokens: None,
                context_window: Some(agent.context_window),
            },
        )
        .await?;
        // Reset provider-actual anchor — last value referred to the
        // pre-compaction prompt and is now stale.
        *last_actual_tokens = None;
        // OnCompaction After — observation; plugins emit metrics or
        // post-process the new projection. Stop does not unwind the
        // compaction (already durable). A plugin may also set
        // `autocontinue = false` via Replace to pause the loop instead of
        // driving the synthetic resume turn — see handling below.
        let after_ctx = OnCompactionCtx {
            session_id: Some(session_id),
            phase: CompactionPhase::After,
            message_count: input.messages.len(),
            autocontinue: true,
        };
        let after_outcome = dispatch(&loop_ctx.handles.hook_chains.on_compaction, after_ctx).await;
        publish_fault_if_any(&loop_ctx.handles.events, Some(session_id), &after_outcome).await;
        // Honor the autocontinue toggle: when a plugin returns Replace
        // with autocontinue=false from the After phase, skip the synthetic
        // resume turn, signal the pause via SessionStatus::Idle, and exit
        // the loop. SessionStatus::Idle is used as a proxy until a
        // dedicated Paused variant lands.
        if let DispatchOutcome::Completed(ref ctx) = after_outcome
            && !ctx.autocontinue
        {
            let _ = loop_ctx
                .handles
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
            return Ok(CompactionFlow::Halt);
        }
        // Post-compaction overflow check.
        //
        // Thread the SAME provider-actual anchor the top-of-loop
        // `should_compact` uses, instead of a hardcoded `None`, so the two
        // checks share one mechanism (use provider-reported tokens when
        // known; otherwise trust the heuristic). At THIS point
        // `last_actual_tokens` was just reset to `None` above because the
        // pre-compaction usage is stale and the compaction turn measured the
        // OLD (large) input, not the new summary+tail projection — so this
        // correctly falls back to the heuristic over `input.messages`. The
        // post-compaction check tolerates a small heuristic overshoot to
        // avoid false aborts when the heuristic and the provider tokenizer
        // disagree.
        if matches!(
            should_compact(&input.messages, agent, *last_actual_tokens, 0),
            CompactDecision::Run { .. }
        ) {
            return Err(CoreError::ContextOverflowAfterCompaction);
        }
        Ok(CompactionFlow::Continue)
    }
}

async fn set_compaction_request_state(
    memory: &Arc<dyn MemoryStore>,
    events: &Arc<dyn crate::adapters::event_sink::EventSink>,
    session_id: SessionId,
    message_id: MessageId,
    part_id: PartId,
    state: CompactionAttemptState,
    summary_message_id: Option<String>,
) -> Result<(), CoreError> {
    memory
        .upsert_part(
            message_id,
            part_id,
            Part::CompactionRequest {
                id: part_id,
                state,
                summary_message_id,
            },
        )
        .await?;
    let _ = events
        .publish(
            AgentEvent::PartUpdated {
                session_id,
                message_id,
                part_id,
            },
            Persistence::Durable,
        )
        .await;
    Ok(())
}

async fn latest_attempt_assistant(
    memory: &Arc<dyn MemoryStore>,
    session_id: SessionId,
    request_message_id: MessageId,
) -> Result<Option<String>, CoreError> {
    let messages = memory.list_messages(session_id).await?;
    let Some(start) = messages
        .iter()
        .position(|message| message.id == request_message_id)
    else {
        return Ok(None);
    };
    Ok(messages[start + 1..]
        .iter()
        .rev()
        .find(|message| message.role == crate::types::message::Role::Assistant)
        .map(|message| message.id.0.to_string()))
}
