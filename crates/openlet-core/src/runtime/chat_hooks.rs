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
/// Run the `OnChatHeaders` chain and return the accumulated headers as a
/// map keyed by header name (last write wins on duplicate names). Empty
/// when no plugin registered the hook or none set a header. The provider
/// merges these over the request, filtering reserved auth-bearing names.
pub(super) async fn run_chat_headers(
    chains: &HookChains,
    events: &Arc<dyn EventSink>,
    session_id: SessionId,
    model: &str,
) -> std::collections::BTreeMap<String, String> {
    if chains.on_chat_headers.is_empty() {
        return std::collections::BTreeMap::new();
    }
    let ctx = OnChatHeadersCtx {
        model: model.to_string(),
        headers: Vec::new(),
    };
    let outcome = dispatch(&chains.on_chat_headers, ctx).await;
    publish_fault_if_any(events, Some(session_id), &outcome).await;
    match outcome {
        DispatchOutcome::Completed(ctx) | DispatchOutcome::Stopped(ctx) => {
            ctx.headers.into_iter().collect()
        }
        DispatchOutcome::Denied { .. } => std::collections::BTreeMap::new(),
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::event_sink::{DeliveredEvent, Persistence};
    use crate::dispatch::HookEntry;
    use crate::hooks::{HookKind, HookResult, Priority};
    use crate::types::event::{AgentEvent, EventFilter};
    use async_trait::async_trait;
    use tokio::sync::broadcast;

    struct NullSink(broadcast::Sender<DeliveredEvent>);

    #[async_trait]
    impl EventSink for NullSink {
        async fn publish(
            &self,
            _ev: AgentEvent,
            _p: Persistence,
        ) -> Result<(), crate::error::EventError> {
            Ok(())
        }
        fn subscribe(&self, _f: EventFilter) -> broadcast::Receiver<DeliveredEvent> {
            self.0.subscribe()
        }
    }

    fn null_sink() -> Arc<dyn EventSink> {
        let (tx, _) = broadcast::channel(4);
        Arc::new(NullSink(tx))
    }

    fn header_hook(name: &'static str, value: &'static str) -> HookEntry<OnChatHeadersCtx> {
        HookEntry {
            manifest_id: "test-plugin".into(),
            priority: Priority(0),
            registration_index: 0,
            kind: HookKind::OnChatHeaders,
            func: Arc::new(move |mut ctx: OnChatHeadersCtx| {
                Box::pin(async move {
                    ctx.headers.push((name.to_string(), value.to_string()));
                    HookResult::Replace(ctx)
                })
            }),
        }
    }

    /// Regression: a plugin that pushes a header through `OnChatHeaders`
    /// must have that header surface in the returned map. Before the fix
    /// `run_chat_headers` dispatched the chain but dropped the mutated
    /// ctx, so plugin headers never reached the provider.
    #[tokio::test]
    async fn plugin_header_round_trips_through_run_chat_headers() {
        let mut chains = HookChains::new();
        chains
            .on_chat_headers
            .push(header_hook("x-trace-id", "abc123"));
        let out = run_chat_headers(&chains, &null_sink(), SessionId::new(), "anthropic/x").await;
        assert_eq!(out.get("x-trace-id").map(String::as_str), Some("abc123"));
    }

    /// Empty by default: no registered hook → no headers, so existing
    /// flows send a vanilla request (the provider merge is a no-op).
    #[tokio::test]
    async fn no_hook_yields_empty_headers() {
        let chains = HookChains::new();
        let out = run_chat_headers(&chains, &null_sink(), SessionId::new(), "anthropic/x").await;
        assert!(out.is_empty());
    }
}
