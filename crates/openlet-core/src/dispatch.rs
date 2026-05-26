//! Hook dispatch — typed [`HookEntry`] storage + ordered runner.
//!
//! Lives in `openlet-core` (not `openlet-plugin-api`) so runtime
//! dispatch sites can call [`dispatch`] without a circular dep.
//! `openlet-plugin-api` re-exports everything for plugin authors.
//!
//! Each hook kind from [`HookKind`] gets its own `Vec<HookEntry<I>>` on
//! [`HookChains`]. The runner walks entries in priority-sorted order and
//! threads the input through `Continue` / `Replace` outcomes, halting on
//! `Stop` or `Deny`. Panics from any single entry are isolated so a
//! buggy plugin cannot crash the server.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::hooks::{
    HookKind, HookResult, Priority,
    io::{
        AfterToolCallCtx, AfterTurnCtx, BeforeToolCallCtx, BeforeTurnCtx, NotificationCtx,
        OnChatHeadersCtx, OnChatMessagesCtx, OnChatParamsCtx, OnCompactionCtx, OnCostTickCtx,
        OnEventCtx, OnMessageCtx, OnPermissionAskCtx, OnSessionStatusCtx, OnStepFinishCtx,
    },
};

/// Hard ceiling on how long a single hook may run. Mirrors the
/// claude-code 5s `timeout` knob and gives a buggy plugin no way to
/// stall a turn indefinitely. Per-hook overrides land in slice 3c.
///
/// (Constant lives in `runner.rs` next to the dispatch loop that uses
/// it.)

/// Future type returned by a hook closure.
pub type HookFuture<I> = Pin<Box<dyn Future<Output = HookResult<I>> + Send + 'static>>;

/// Closure stored on a [`HookEntry`]. Receives the mutable hook context
/// by value (the runner passes `I` in/out via the [`HookResult`] enum).
pub type HookFn<I> = Arc<dyn Fn(I) -> HookFuture<I> + Send + Sync + 'static>;

/// One registered hook handler.
#[derive(Clone)]
pub struct HookEntry<I> {
    pub manifest_id: String,
    pub priority: Priority,
    /// Insertion index — last tiebreaker after priority + manifest id.
    pub registration_index: usize,
    pub kind: HookKind,
    pub func: HookFn<I>,
}

impl<I> std::fmt::Debug for HookEntry<I> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HookEntry")
            .field("manifest_id", &self.manifest_id)
            .field("priority", &self.priority)
            .field("registration_index", &self.registration_index)
            .field("kind", &self.kind)
            .finish()
    }
}

/// Sorted hook chains, one per [`HookKind`]. Built by
/// `PluginRegistry::install_all` after every plugin's `install` returns.
#[derive(Default, Debug)]
pub struct HookChains {
    pub before_turn: Vec<HookEntry<BeforeTurnCtx>>,
    pub after_turn: Vec<HookEntry<AfterTurnCtx>>,
    pub on_chat_params: Vec<HookEntry<OnChatParamsCtx>>,
    pub on_chat_messages: Vec<HookEntry<OnChatMessagesCtx>>,
    pub on_chat_headers: Vec<HookEntry<OnChatHeadersCtx>>,
    pub before_tool_call: Vec<HookEntry<BeforeToolCallCtx>>,
    pub after_tool_call: Vec<HookEntry<AfterToolCallCtx>>,
    pub on_permission_ask: Vec<HookEntry<OnPermissionAskCtx>>,
    pub on_message: Vec<HookEntry<OnMessageCtx>>,
    pub on_cost_tick: Vec<HookEntry<OnCostTickCtx>>,
    pub on_step_finish: Vec<HookEntry<OnStepFinishCtx>>,
    pub on_compaction: Vec<HookEntry<OnCompactionCtx>>,
    pub on_session_status: Vec<HookEntry<OnSessionStatusCtx>>,
    pub on_event: Vec<HookEntry<OnEventCtx>>,
    pub notification: Vec<HookEntry<NotificationCtx>>,
}

impl HookChains {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sort every chain in canonical order: priority desc, manifest_id
    /// asc, registration_index asc. Idempotent.
    pub fn sort_all(&mut self) {
        macro_rules! sort_each {
            ($($field:ident),+ $(,)?) => {
                $(sort_chain(&mut self.$field);)+
            };
        }
        sort_each!(
            before_turn,
            after_turn,
            on_chat_params,
            on_chat_messages,
            on_chat_headers,
            before_tool_call,
            after_tool_call,
            on_permission_ask,
            on_message,
            on_cost_tick,
            on_step_finish,
            on_compaction,
            on_session_status,
            on_event,
            notification,
        );
    }
}

fn sort_chain<I>(chain: &mut [HookEntry<I>]) {
    chain.sort_by(|a, b| {
        // Higher priority first.
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.manifest_id.cmp(&b.manifest_id))
            .then_with(|| a.registration_index.cmp(&b.registration_index))
    });
}

/// Outcome of running a chain end-to-end.
#[derive(Debug)]
pub enum DispatchOutcome<I> {
    /// Chain ran to completion. `Continue` / `Replace` threaded `I`.
    Completed(I),
    /// A hook returned `Stop`. Subsequent hooks not invoked.
    Stopped(I),
    /// A hook returned `Deny`, OR a synthetic deny from panic/timeout.
    /// `plugin_fault.is_some()` ⇒ the deny came from a fault (not the
    /// plugin's explicit return). Runtime sites use that to decide
    /// whether to emit `AgentEvent::PluginError` for cloud-grep telemetry.
    Denied {
        reason: String,
        feedback: Option<String>,
        plugin_fault: Option<PluginFault>,
    },
}

/// Closed taxonomy for synthetic-deny causes — emitted alongside
/// `tracing::error!` so cloud users can grep `event.kind = plugin_error`
/// without parsing log strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultKind {
    ConstructionPanic,
    PollPanic,
    Timeout,
}

impl FaultKind {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ConstructionPanic => "construction_panic",
            Self::PollPanic => "poll_panic",
            Self::Timeout => "timeout",
        }
    }
}

/// Provenance for a synthetic `Denied` outcome. Carries the offending
/// plugin + hook so the caller can emit a structured event without
/// parsing the human-readable `reason` string.
#[derive(Debug, Clone)]
pub struct PluginFault {
    pub plugin_id: String,
    pub hook: HookKind,
    pub kind: FaultKind,
    pub message: String,
}

/// Run a hook chain. Each entry is invoked sequentially; the input
/// threads through `Continue`/`Replace` and the chain halts on `Stop` or
pub mod runner;
pub use runner::{dispatch, dispatch_event};

/// Build an [`AgentEvent::PluginError`] from a [`PluginFault`].
/// Runtime dispatch sites publish the result on durable persistence so
/// cloud operators can grep `kind = plugin.error` without parsing logs.
/// `hook` uses the stable snake_case label from [`HookKind::as_str`] so
/// renaming a variant doesn't break downstream dashboards.
#[must_use]
pub fn plugin_error_event(
    session_id: Option<crate::types::session::SessionId>,
    fault: &PluginFault,
) -> crate::types::event::AgentEvent {
    crate::types::event::AgentEvent::PluginError {
        session_id,
        plugin_id: fault.plugin_id.clone(),
        hook: format!("{}|{}", fault.hook.as_str(), fault.kind.as_str()),
        message: fault.message.clone(),
    }
}

/// If `outcome` is a synthetic deny (panic / timeout), publish a
/// `PluginError` event durably so cloud operators see the fault. Used
/// by every runtime dispatch site so observation-only chains
/// (`AfterTurn`, `OnStepFinish`, `OnEvent`, …) don't silently swallow
/// faults.
pub async fn publish_fault_if_any<I>(
    events: &std::sync::Arc<dyn crate::adapters::event_sink::EventSink>,
    session_id: Option<crate::types::session::SessionId>,
    outcome: &DispatchOutcome<I>,
) {
    if let DispatchOutcome::Denied {
        plugin_fault: Some(fault),
        ..
    } = outcome
    {
        let _ = events
            .publish(
                plugin_error_event(session_id, fault),
                crate::adapters::event_sink::Persistence::Durable,
            )
            .await;
    }
}

/// Handle a `Denied` outcome from a hook chain: publish the plugin
/// fault event (if any) and emit a tracing warn with the deny reason
/// + feedback. Centralises the 3-site pattern in `conversation.rs` /
///   `turn_loop.rs` where a denied chat/turn hook halts the loop.
pub async fn publish_denied_warn(
    events: &std::sync::Arc<dyn crate::adapters::event_sink::EventSink>,
    session_id: Option<crate::types::session::SessionId>,
    hook_label: &'static str,
    reason: &str,
    feedback: &Option<String>,
    plugin_fault: Option<&PluginFault>,
) {
    if let Some(fault) = plugin_fault {
        let _ = events
            .publish(
                plugin_error_event(session_id, fault),
                crate::adapters::event_sink::Persistence::Durable,
            )
            .await;
    }
    tracing::warn!(
        hook = hook_label,
        reason = %reason,
        feedback = ?feedback,
        "hook denied; halting turn"
    );
}
