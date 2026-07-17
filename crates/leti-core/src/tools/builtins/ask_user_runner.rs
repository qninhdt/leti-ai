//! Runner for the `ask_user` tool. Split from `ask_user.rs` so each
//! file stays under 200 lines and the validation + suspend/resume logic
//! has room to breathe.

use std::time::Duration;

use crate::adapters::event_sink::Persistence;
use crate::adapters::tool_executor::ToolCtx;
use crate::error::ToolError;
use crate::runtime::question_registry::{CancelReason, QuestionId};
use crate::tools::builtins::ask_user::{
    AskUserInput, AskUserOutput, MAX_HEADER_LEN, MAX_OPTIONS, MIN_OPTIONS,
};
use crate::types::event::{AgentEvent, AskOption};
use tokio::sync::oneshot;

/// Stable error codes the model can pattern-match on. Re-used by tests.
pub const ERR_UNAVAILABLE: &str = "user_questions_unavailable_in_session";
pub const ERR_ALREADY_PENDING: &str = "question_already_pending";
pub const ERR_CANCELLED: &str = "question_cancelled";
pub const ERR_INVALID_HEADER: &str = "ask_user_invalid_header";
pub const ERR_INVALID_OPTIONS: &str = "ask_user_invalid_options";
pub const ERR_INVALID_QUESTION: &str = "ask_user_invalid_question";

/// Run the tool. Order:
/// 1. validate input (header length, option count)
/// 2. read session capability — synchronous error if `user_questions=false`
/// 3. claim per-session pending slot
/// 4. register oneshot, emit `QuestionRequested`, await reply with timeout
/// 5. release slot in every exit path (success, timeout, cancel)
pub(super) async fn run(
    timeout: Duration,
    ctx: ToolCtx,
    input: AskUserInput,
) -> Result<AskUserOutput, ToolError> {
    validate_input(&input)?;

    // Capability gate. Headless sessions never block.
    let meta = ctx
        .memory
        .get_session(ctx.session_id)
        .await
        .map_err(|e| ToolError::Io(format!("memory: {e}")))?
        .ok_or_else(|| ToolError::Io("session not found".to_string()))?;
    if !meta.capabilities.user_questions {
        return Err(ToolError::InvalidInput(ERR_UNAVAILABLE.to_string()));
    }

    // Per-session 1-pending cap.
    if !ctx.questions.try_claim_session_slot(ctx.session_id) {
        return Err(ToolError::InvalidInput(ERR_ALREADY_PENDING.to_string()));
    }
    // RAII guard so every exit path releases the slot.
    let _slot = SessionSlotGuard::new(&ctx);

    let qid = QuestionId::new();
    let (tx, rx) = oneshot::channel::<Vec<usize>>();
    ctx.questions.register(qid, ctx.session_id, tx);

    // Emit the request event so the SSE stream wakes the frontend.
    let options: Vec<AskOption> = input
        .options
        .iter()
        .map(|o| AskOption {
            label: o.label.clone(),
            description: o.description.clone(),
        })
        .collect();
    ctx.events
        .publish(
            AgentEvent::QuestionRequested {
                session_id: ctx.session_id,
                question_id: qid,
                header: input.header.clone(),
                question: input.question.clone(),
                options: options.clone(),
                multi_select: input.multi_select,
            },
            Persistence::Durable,
        )
        .await
        .map_err(|e| ToolError::Io(format!("publish QuestionRequested: {e}")))?;

    // Suspend on the receiver until one of: reply / timeout / session cancel.
    let selected = await_reply(timeout, &ctx, qid, rx).await?;

    // Validate selection bounds before handing back to the model.
    if selected.iter().any(|i| *i >= input.options.len()) {
        return Err(ToolError::InvalidInput(
            "ask_user_selection_out_of_range".to_string(),
        ));
    }
    if !input.multi_select && selected.len() != 1 {
        return Err(ToolError::InvalidInput(
            "ask_user_single_select_requires_one".to_string(),
        ));
    }

    let selected_labels = selected
        .iter()
        .filter_map(|i| input.options.get(*i).map(|o| o.label.clone()))
        .collect();

    Ok(AskUserOutput {
        question_id: qid.to_string(),
        selected,
        selected_labels,
    })
}

fn validate_input(input: &AskUserInput) -> Result<(), ToolError> {
    if input.header.is_empty() || input.header.chars().count() > MAX_HEADER_LEN {
        return Err(ToolError::InvalidInput(ERR_INVALID_HEADER.to_string()));
    }
    if input.question.trim().is_empty() {
        return Err(ToolError::InvalidInput(ERR_INVALID_QUESTION.to_string()));
    }
    if input.options.len() < MIN_OPTIONS || input.options.len() > MAX_OPTIONS {
        return Err(ToolError::InvalidInput(ERR_INVALID_OPTIONS.to_string()));
    }
    if input.options.iter().any(|o| o.label.trim().is_empty()) {
        return Err(ToolError::InvalidInput(ERR_INVALID_OPTIONS.to_string()));
    }
    Ok(())
}

async fn await_reply(
    timeout: Duration,
    ctx: &ToolCtx,
    qid: QuestionId,
    mut rx: oneshot::Receiver<Vec<usize>>,
) -> Result<Vec<usize>, ToolError> {
    // Honor an ALREADY-DELIVERED answer first. If the user's reply
    // landed on the oneshot BEFORE we reach the select, drain it and
    // return it. Without this, a `cancel` that fires in the same scheduler
    // tick would preempt a reply the user already gave (the biased select
    // always polls `cancel` first), silently dropping a legitimate answer.
    //
    // We do NOT weaken cancellation: `cancel` = `CancelReason::SessionEnding`
    // is an operator kill / consent revocation. It MUST still win over an
    // answer that has NOT yet arrived. So we only short-circuit on an answer
    // that is already buffered; otherwise we fall through to the still-`biased`
    // select where cancel is preferred. We never coin-flip consent.
    if let Some(buffered) = drain_buffered_answer(&mut rx) {
        return buffered;
    }

    // Race the reply against (a) the cancellation token from the loop
    // (session DELETE / abort) and (b) the timeout. Whichever fires
    // first deregisters the entry. `cancel` stays preferred (biased) so a
    // not-yet-arrived answer cannot beat a revocation.
    tokio::select! {
        biased;
        () = ctx.cancel.cancelled() => {
            ctx.questions.cancel(qid, CancelReason::SessionEnding);
            Err(ToolError::InvalidInput(ERR_CANCELLED.to_string()))
        }
        () = tokio::time::sleep(timeout) => {
            ctx.questions.cancel(qid, CancelReason::Operator);
            Err(ToolError::Timeout)
        }
        result = &mut rx => match result {
            Ok(selected) => Ok(selected),
            Err(_) => Err(ToolError::InvalidInput(ERR_CANCELLED.to_string())),
        }
    }
}

/// Non-blocking drain of an already-delivered answer.
///
/// Returns:
/// - `Some(Ok(selected))` when an answer is already buffered on the oneshot
///   (honored before the cancel-biased select can preempt it),
/// - `Some(Err(cancelled))` when the sender was dropped without sending
///   (mirrors the `Err(_)` arm of the select's receiver branch),
/// - `None` when no answer is buffered yet (caller falls through to the
///   still-`biased` cancel-vs-rx-vs-timeout select).
fn drain_buffered_answer(
    rx: &mut oneshot::Receiver<Vec<usize>>,
) -> Option<Result<Vec<usize>, ToolError>> {
    match rx.try_recv() {
        Ok(selected) => Some(Ok(selected)),
        Err(oneshot::error::TryRecvError::Empty) => None,
        Err(oneshot::error::TryRecvError::Closed) => {
            Some(Err(ToolError::InvalidInput(ERR_CANCELLED.to_string())))
        }
    }
}

struct SessionSlotGuard<'a> {
    ctx: &'a ToolCtx,
}

impl<'a> SessionSlotGuard<'a> {
    fn new(ctx: &'a ToolCtx) -> Self {
        Self { ctx }
    }
}

impl Drop for SessionSlotGuard<'_> {
    fn drop(&mut self) {
        self.ctx.questions.remove_session_slot(self.ctx.session_id);
    }
}

#[cfg(test)]
mod h6_buffered_answer_tests {
    //! An answer that has ALREADY been delivered before a cancel must
    //! be honored, not dropped by the cancel-biased select. The drain helper
    //! is the deterministic core of that guarantee.
    use super::*;
    use tokio::sync::oneshot;

    #[test]
    fn buffered_answer_is_returned_even_when_cancel_would_be_preferred() {
        let (tx, mut rx) = oneshot::channel::<Vec<usize>>();
        // Answer delivered BEFORE we drain.
        tx.send(vec![2]).unwrap();
        match drain_buffered_answer(&mut rx) {
            Some(Ok(selected)) => assert_eq!(selected, vec![2]),
            other => panic!("expected buffered answer to be honored, got {other:?}"),
        }
    }

    #[test]
    fn no_buffered_answer_falls_through_to_select() {
        let (_tx, mut rx) = oneshot::channel::<Vec<usize>>();
        // Sender alive, nothing sent — must fall through (None) so the
        // biased select (cancel-preferred) runs.
        assert!(
            drain_buffered_answer(&mut rx).is_none(),
            "empty channel must fall through to the cancel-biased select"
        );
    }

    #[test]
    fn dropped_sender_maps_to_cancelled() {
        let (tx, mut rx) = oneshot::channel::<Vec<usize>>();
        drop(tx);
        match drain_buffered_answer(&mut rx) {
            Some(Err(ToolError::InvalidInput(code))) => assert_eq!(code, ERR_CANCELLED),
            other => panic!("expected cancelled error on closed channel, got {other:?}"),
        }
    }
}
