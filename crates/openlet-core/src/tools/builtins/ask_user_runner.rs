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

    // F1.1: capability gate. Headless sessions never block.
    let meta = ctx
        .memory
        .get_session(ctx.session_id)
        .await
        .map_err(|e| ToolError::Io(format!("memory: {e}")))?
        .ok_or_else(|| ToolError::Io("session not found".to_string()))?;
    if !meta.capabilities.user_questions {
        return Err(ToolError::InvalidInput(ERR_UNAVAILABLE.to_string()));
    }

    // F1.4: per-session 1-pending cap.
    if !ctx.questions.try_claim_session_slot(ctx.session_id) {
        return Err(ToolError::InvalidInput(ERR_ALREADY_PENDING.to_string()));
    }
    // RAII guard so every exit path releases the slot.
    let _slot = SessionSlotGuard::new(&ctx);

    let qid = QuestionId::new();
    let (tx, rx) = oneshot::channel::<Vec<usize>>();
    ctx.questions.register(qid, tx);

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
    // Race the reply against (a) the cancellation token from the loop
    // (session DELETE / abort) and (b) the timeout. Whichever fires
    // first deregisters the entry.
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
        self.ctx.questions.release_session_slot(self.ctx.session_id);
    }
}
