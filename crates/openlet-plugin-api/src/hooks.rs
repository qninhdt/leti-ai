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
