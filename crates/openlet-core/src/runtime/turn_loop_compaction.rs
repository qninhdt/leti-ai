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

use super::ConversationRuntime;
use super::TurnInput;
use super::turn_loop::LoopContext;
use super::turn_loop_helpers::{collect_assistant_text, project_session_messages};

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
        let before_outcome = dispatch(&loop_ctx.hook_chains.on_compaction, before_ctx).await;
        publish_fault_if_any(&loop_ctx.events, Some(session_id), &before_outcome).await;
        if matches!(
            before_outcome,
            DispatchOutcome::Stopped(_) | DispatchOutcome::Denied { .. }
        ) {
            return Ok(CompactionFlow::Continue);
        }
        // Determine which existing messages will be superseded.
        let messages = memory.list_messages(session_id).await?;
        let mut superseded = superseded_messages(&messages, keep);
        let original_tokens = estimate_conversation_tokens(&input.messages) as u32;
        // Append synthetic user message asking for summary. The synthetic
        // id is added to `superseded` so the projection substitutes the
        // summary in its place — otherwise the next turn would see the
        // literal "Summarize the conversation history above" prompt it
        // never issued.
        let synth_id = append_synthetic_request(memory, &loop_ctx.events, session_id).await?;
        // Build a one-shot compaction projection and run a turn. The
        // result text becomes Part::Compaction.
        let mut compact_input = input.clone();
        compact_input.messages = build_compaction_projection(&input.messages, keep);
        compact_input.tools = Vec::new();
        // If the compaction turn fails or is cancelled, the synthetic
        // "Summarize the conversation above" message remains in storage as
        // a real user turn. Roll it back by superseding it in a no-op
        // compaction part rather than leaving it visible to subsequent
        // projections.
        let outcome = match self.run_turn(compact_input, cancel).await {
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
        let summary =
            collect_assistant_text(memory, session_id, outcome.assistant_message_id).await?;
        // Refuse to persist an empty summary — that would supersede older
        // messages with a blank string and silently lose history. Roll
        // back the synthetic request and the empty assistant turn, then
        // bubble the failure up so the caller can retry.
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
        // The compaction-turn's assistant message holds the verbatim
        // summary as Part::Text; substitute it via the Compaction part on
        // subsequent projections so the model doesn't see the summary twice.
        superseded.push(outcome.assistant_message_id);
        let _comp_id = append_compaction_part(
            memory,
            &loop_ctx.events,
            session_id,
            summary,
            superseded,
            original_tokens,
        )
        .await?;
        // Re-project so the next turn sees the summary in place of the
        // compacted messages.
        input.messages = project_session_messages(memory, session_id).await?;
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
        let after_outcome = dispatch(&loop_ctx.hook_chains.on_compaction, after_ctx).await;
        publish_fault_if_any(&loop_ctx.events, Some(session_id), &after_outcome).await;
        // Honor the autocontinue toggle: when a plugin returns Replace
        // with autocontinue=false from the After phase, skip the synthetic
        // resume turn, signal the pause via SessionStatus::Idle, and exit
        // the loop. SessionStatus::Idle is used as a proxy until a
        // dedicated Paused variant lands.
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
                return Ok(CompactionFlow::Halt);
            }
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
