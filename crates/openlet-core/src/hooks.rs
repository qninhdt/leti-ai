//! Typed plugin hook surface — kinds, results, priorities, and the
//! per-kind context structs that runtime dispatch sites construct.
//!
//! Lives in `openlet-core` (not `openlet-plugin-api`) because runtime
//! dispatch sites in this crate need to construct `*Ctx` values to call
//! [`crate::dispatch::dispatch`]. `openlet-plugin-api` re-exports
//! everything here so plugin authors only ever import from one crate.

use serde::{Deserialize, Serialize};

/// Hook ordering priority. Higher runs first; ties broken by manifest id
/// (lex asc), then registration order. Default 50.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Priority(pub u8);

impl Default for Priority {
    fn default() -> Self {
        Self(50)
    }
}

/// Closed enum of hook kinds — drives capability declaration so the
/// runtime can skip uninvoked hook chains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookKind {
    BeforeTurn,
    AfterTurn,
    OnChatParams,
    OnChatMessages,
    OnChatHeaders,
    BeforeToolCall,
    AfterToolCall,
    OnPermissionAsk,
    OnMessage,
    OnCostTick,
    OnStepFinish,
    OnCompaction,
    OnSessionStatus,
    OnEvent,
}

impl HookKind {
    /// Stable snake_case label matching the `serde(rename_all)` form.
    /// Used in `AgentEvent::PluginError` so cloud dashboards keep parsing
    /// the same string when variants are added or renumbered.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::BeforeTurn => "before_turn",
            Self::AfterTurn => "after_turn",
            Self::OnChatParams => "on_chat_params",
            Self::OnChatMessages => "on_chat_messages",
            Self::OnChatHeaders => "on_chat_headers",
            Self::BeforeToolCall => "before_tool_call",
            Self::AfterToolCall => "after_tool_call",
            Self::OnPermissionAsk => "on_permission_ask",
            Self::OnMessage => "on_message",
            Self::OnCostTick => "on_cost_tick",
            Self::OnStepFinish => "on_step_finish",
            Self::OnCompaction => "on_compaction",
            Self::OnSessionStatus => "on_session_status",
            Self::OnEvent => "on_event",
        }
    }
}

/// Outcome of a hook invocation. Fixes opencode's mutate-in-place footgun:
/// hooks must be explicit about whether they short-circuit, override, or
/// merely observe.
///
/// **Continue vs Replace.** Both pass `T` to the next hook in the chain.
/// `Replace` additionally records an audit trail entry — the dispatcher
/// logs the manifest id of the hook that produced it so two plugins
/// disagreeing on the same value leave a forensic trace. `Replace` is NOT
/// terminal; if termination is desired, use `Stop`.
#[derive(Debug)]
pub enum HookResult<T> {
    /// Pass T to the next hook in the chain. No audit log.
    Continue(T),
    /// Pass T to next hook AND log this hook as the authoritative source
    /// of T. Useful when a plugin overrides a value other plugins set.
    Replace(T),
    /// Halt chain immediately, T is final.
    Stop(T),
    /// Short-circuit deny — used by permission/before_tool hooks.
    Deny {
        reason: String,
        feedback: Option<String>,
    },
}

/// Hook input/output context structs — one per [`HookKind`].
///
/// Fields are by-value because [`crate::dispatch::dispatch`] threads
/// `I` through `Continue`/`Replace`. Runtime call sites clone what they
/// need into the ctx, dispatch the chain, then unwrap mutated fields
/// from the returned ctx. `Send + 'static` is the only hard bound.
pub mod io {
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
}
