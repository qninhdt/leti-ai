//! Chat-hook dispatch helpers for `ConversationRuntime::run_turn`.
//!
//! Centralises the OnChatParams / OnChatMessages / OnChatHeaders chain
//! invocation: build ctx → dispatch → handle Denied (publish + warn +
//! halt) → return mutated ctx. Each hook's empty-chain fast path is
//! preserved so we never construct a ctx clone when no plugin is
//! registered.

use std::sync::Arc;

use crate::adapters::event_sink::EventSink;
use crate::dispatch::{
    DispatchOutcome, HookChains, dispatch, publish_denied_warn, publish_fault_if_any,
};
use crate::error::{CoreError, ProviderError};
use crate::hooks::io::{OnChatHeadersCtx, OnChatMessagesCtx, OnChatParamsCtx};
use crate::projection::LlmMessage;
use crate::types::session::SessionId;

/// Run the OnChatParams chain. Returns the (possibly mutated) ctx; on
/// Denied surfaces a `Provider(Cancelled)` error after publishing the
/// fault + warn so the runtime halts the turn cleanly.
pub(super) async fn run_chat_params(
    chains: &HookChains,
    events: &Arc<dyn EventSink>,
    session_id: SessionId,
    initial: OnChatParamsCtx,
) -> Result<OnChatParamsCtx, CoreError> {
    if chains.on_chat_params.is_empty() {
        return Ok(initial);
    }
    match dispatch(&chains.on_chat_params, initial).await {
        DispatchOutcome::Completed(c) | DispatchOutcome::Stopped(c) => Ok(c),
        DispatchOutcome::Denied {
            reason,
            feedback,
            plugin_fault,
        } => {
            publish_denied_warn(
                events,
                Some(session_id),
                "on_chat_params",
                &reason,
                &feedback,
                plugin_fault.as_ref(),
            )
            .await;
            Err(CoreError::Provider(ProviderError::Cancelled))
        }
    }
}

/// Run the OnChatMessages chain. Returns the (possibly mutated) ctx;
/// see `run_chat_params` for Denied semantics.
pub(super) async fn run_chat_messages(
    chains: &HookChains,
    events: &Arc<dyn EventSink>,
    session_id: SessionId,
    initial: OnChatMessagesCtx,
) -> Result<OnChatMessagesCtx, CoreError> {
    if chains.on_chat_messages.is_empty() {
        return Ok(initial);
    }
    match dispatch(&chains.on_chat_messages, initial).await {
        DispatchOutcome::Completed(c) | DispatchOutcome::Stopped(c) => Ok(c),
        DispatchOutcome::Denied {
            reason,
            feedback,
            plugin_fault,
        } => {
            publish_denied_warn(
                events,
                Some(session_id),
                "on_chat_messages",
                &reason,
                &feedback,
                plugin_fault.as_ref(),
            )
            .await;
            Err(CoreError::Provider(ProviderError::Cancelled))
        }
    }
}

/// Run the OnChatHeaders chain (observation-only today; phase 4
/// widens `ModelProvider::chat_stream` to consume the mutated
/// headers). Empty-chain fast path skips ctx construction.
pub(super) async fn run_chat_headers(
    chains: &HookChains,
    events: &Arc<dyn EventSink>,
    session_id: SessionId,
    model: &str,
) {
    if chains.on_chat_headers.is_empty() {
        return;
    }
    let ctx = OnChatHeadersCtx {
        model: model.to_string(),
        headers: Vec::new(),
    };
    let outcome = dispatch(&chains.on_chat_headers, ctx).await;
    publish_fault_if_any(events, Some(session_id), &outcome).await;
}

/// Builder for the initial `OnChatMessagesCtx` — wraps the
/// model/system_prompt/messages triple so the call site doesn't have
/// to spell it out twice (once for the empty-chain path, once for the
/// dispatched path). The empty-chain check now lives inside
/// `run_chat_messages`, so the caller only builds the ctx once.
#[must_use]
pub(super) fn build_messages_ctx(
    model: String,
    system_prompt: Option<String>,
    messages: Vec<LlmMessage>,
) -> OnChatMessagesCtx {
    OnChatMessagesCtx {
        model,
        system_prompt,
        messages,
    }
}
