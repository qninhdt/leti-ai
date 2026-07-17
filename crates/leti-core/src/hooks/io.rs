//! Per-hook context structs (`*Ctx`).
//!
//! Extracted from the parent `hooks.rs` so the kind/priority/result
//! enums live next to their dispatch wiring while every ctx type lives
//! together. Plugin authors import these via
//! `leti_plugin_api::prelude::*` (which re-exports them) — no
//! direct path change here.

use crate::adapters::model_provider::FinishReason;
use crate::projection::LlmMessage;
use crate::tools::{ToolDispatchResult, ToolInvocation};
use crate::types::event::{AgentEvent, Usage};
use crate::types::message::Message;
use crate::types::permission::{Decision, PermissionRequest};
use crate::types::session::{SessionId, SessionStatus};
use rust_decimal::Decimal;

/// Fired at the start of every model turn. Stop halts the loop with
/// `finish_reason = halted`; Deny feeds a synthetic tool-result back
/// to the model (CorrectedError pattern).
#[derive(Debug, Default)]
pub struct BeforeTurnCtx {
    pub session_id: Option<SessionId>,
    pub turn_index: u32,
    pub message_count: usize,
}

/// Fired after a model turn completes (assistant message stored,
/// tool calls dispatched). Carries the final usage so observers can
/// flush telemetry without re-deriving cost.
#[derive(Debug, Default)]
pub struct AfterTurnCtx {
    pub session_id: Option<SessionId>,
    pub turn_index: u32,
    pub finish_reason: Option<FinishReason>,
    pub usage: Option<Usage>,
    pub cost_usd: Option<Decimal>,
}

/// Mutate provider sampling params before the request goes out.
/// `None` means "use provider/model default."
#[derive(Debug, Default)]
pub struct OnChatParamsCtx {
    pub model: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

/// Rewrite the message list passed to the provider — compaction
/// plugins, ablation testing, prompt-prefix injection. Replacing
/// `messages` thread-through is authoritative.
#[derive(Debug, Default)]
pub struct OnChatMessagesCtx {
    pub model: String,
    pub system_prompt: Option<String>,
    pub messages: Vec<LlmMessage>,
}

/// Inject auth headers / tracing headers per provider call.
/// Slice 3b ships with an empty `headers` map; phase 4 widens
/// `ModelProvider::chat_stream` to consume it.
#[derive(Debug, Default)]
pub struct OnChatHeadersCtx {
    pub model: String,
    pub headers: Vec<(String, String)>,
}

/// Mutate tool args before invocation. `Replace` swaps the
/// invocation's args; `Deny { reason, feedback }` short-circuits to
/// a CorrectedError-style synthetic tool result fed back to the
/// model (does not run the tool).
#[derive(Debug, Default)]
pub struct BeforeToolCallCtx {
    pub session_id: Option<SessionId>,
    pub invocation: Option<ToolInvocation>,
}

/// Mutate tool output. `Replace` substitutes the dispatch result —
/// useful for masking secrets or rewriting downstream display.
#[derive(Debug, Default)]
pub struct AfterToolCallCtx {
    pub session_id: Option<SessionId>,
    pub invocation: Option<ToolInvocation>,
    pub result: Option<ToolDispatchResult>,
}

/// Override the permission decision before the ruleset is consulted.
/// `Replace` returns the override decision; `Continue` falls through
/// to the configured ruleset.
#[derive(Debug, Default)]
pub struct OnPermissionAskCtx {
    pub request: Option<PermissionRequest>,
    pub decision: Option<Decision>,
}

/// Audit-log every appended message. `Stop` is honored but does NOT
/// undo the storage write — the event already happened. Use Stop
/// only to skip downstream observers.
#[derive(Debug, Default)]
pub struct OnMessageCtx {
    pub session_id: Option<SessionId>,
    pub message: Option<Message>,
}

/// Per-step cost delta. `Stop` aborts the loop with `finish_reason =
/// halted` — quota plugin path. `delta_usd` is the just-incurred
/// step cost; `total_usd` is the cumulative session cost.
#[derive(Debug, Default)]
pub struct OnCostTickCtx {
    pub session_id: Option<SessionId>,
    pub model: String,
    pub delta_usd: Option<Decimal>,
    pub total_usd: Decimal,
    pub usage: Option<Usage>,
}

/// Anomaly detection at the step boundary. `step_index` counts
/// model→tools→model trips; `finish_reason` is set on the final
/// step of a turn.
#[derive(Debug, Default)]
pub struct OnStepFinishCtx {
    pub session_id: Option<SessionId>,
    pub step_index: u32,
    pub finish_reason: Option<FinishReason>,
    pub usage: Option<Usage>,
}

/// Compaction phase marker. `Before` fires before the compactor
/// builds its summary prompt; `After` fires after the compaction
/// step has rewritten history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompactionPhase {
    #[default]
    Before,
    After,
}

/// Custom compaction prompts / per-tenant retention rules. Stop
/// halts compaction before it runs (Before phase) or before
/// post-processing (After phase).
#[derive(Debug)]
pub struct OnCompactionCtx {
    pub session_id: Option<SessionId>,
    pub phase: CompactionPhase,
    pub message_count: usize,
    /// When `true` (default), the runtime continues with the next
    /// model turn after compaction completes. A plugin observing the
    /// `After` phase may return `Replace` with `autocontinue = false`
    /// to pause the loop instead — the runtime then emits
    /// `SessionStatus::Idle` as a proxy for a paused state (a future
    /// PR may introduce a dedicated `Paused` variant) and returns
    /// from `run_loop` without driving another model turn. Ignored on
    /// the `Before` phase: the toggle's whole point is gating the
    /// post-compaction continuation.
    pub autocontinue: bool,
}

impl Default for OnCompactionCtx {
    fn default() -> Self {
        Self {
            session_id: None,
            phase: CompactionPhase::default(),
            message_count: 0,
            autocontinue: true,
        }
    }
}

/// Session lifecycle transitions — cleanup, notifications. Fires
/// once per status change; runtime advances even if a hook returns
/// Stop (status is already persisted).
#[derive(Debug, Default)]
pub struct OnSessionStatusCtx {
    pub session_id: Option<SessionId>,
    pub status: Option<SessionStatus>,
}

/// Bus firehose for observability. Slice 3b wraps dispatch at
/// `EventSink::publish` call sites with a specialized runner that
/// downgrades `Stop`/`Deny` to `Continue` so a buggy plugin cannot
/// swallow events for downstream observers. Generic [`dispatch`] in
/// this crate honors all four outcomes uniformly.
///
/// [`dispatch`]: crate::dispatch::dispatch
#[derive(Debug, Default)]
pub struct OnEventCtx {
    pub event: Option<AgentEvent>,
}

/// Severity for [`NotificationCtx`]. Maps to UI banner styling and
/// integrator log level. `Error` does NOT terminate the turn — the
/// notification surface is observation-only; quota / safety stops
/// still go through `OnCostTick` / `BeforeToolCall::Deny`.
pub use crate::types::event::NotificationLevel;

/// User-facing notification emitted by a plugin via
/// `PluginContext::emit_notification`. Other notification hooks
/// observe; SSE event `NotificationEmitted` fans out to clients.
///
/// `body` is redacted by the `SecretRedactor` (in `leti-adapters`) before
/// SSE emission. Per-session rate limiting (10/sec cumulative across
/// plugins) caps misbehaving plugins from flooding the channel —
/// overflow drops the notification and emits a tracing warn.
#[derive(Debug, Default)]
pub struct NotificationCtx {
    pub session_id: Option<SessionId>,
    pub level: NotificationLevel,
    pub title: String,
    pub body: String,
    /// Plugin id of the original emitter — set by the runtime, not
    /// the plugin. Read-only inside hook closures.
    pub source_plugin: String,
}
