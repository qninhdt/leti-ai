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
    /// Plugin-emitted user-facing notification (cloud TUI banner,
    /// integrator's webhook, audit-trail flag). Fan-out only — the
    /// chain runs after `PluginContext::emit_notification` so other
    /// plugins can observe / mutate / suppress.
    Notification,
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
            Self::Notification => "notification",
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
pub mod io;
