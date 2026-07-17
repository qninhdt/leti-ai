//! Conversation runtime — orchestrator + pure-logic helpers.
//!
//! The pure helpers (`processor`, `doom_guard`, `cost`) sit alongside
//! the `ConversationRuntime` orchestrator (`conversation`) and the
//! streaming-id bookkeeping (`turn_stream`) that bridges `Processor`
//! to `MemoryStore` + `EventSink`.

pub mod agent_allowlist;
pub mod attachments;
pub(crate) mod chat_hooks;
pub mod compaction;
pub mod conversation;
pub mod cost;
pub mod doom_guard;
pub mod handles;
pub mod injected_permission;
pub(crate) mod persist;
pub mod processor;
pub mod prompt;
pub mod question_registry;
pub mod reminders;
pub mod request_prep;
pub mod retry;
pub mod subagent;
pub mod token_estimate;
pub mod turn_ext;
pub mod turn_loop;
mod turn_loop_compaction;
mod turn_loop_helpers;
mod turn_stream;

pub use compaction::{
    COMPACTION_REQUEST, CompactDecision, PRESERVE_RECENT, build_compaction_projection,
    should_compact, superseded_messages,
};
pub use conversation::{ConversationRuntime, RuntimeConfig, TurnInput, TurnOutcome};
pub use cost::{compute_cost, format_usd};
pub use doom_guard::{DoomVerdict, ToolCallSig};
pub use handles::RuntimeHandles;
pub use processor::{Processor, ProcessorEvent, ProcessorPart, ProcessorState};
pub use prompt::{compose_system_prompt, select_provider_prompt};
pub use question_registry::{CancelReason, QuestionId, QuestionRegistry, ResolveError};
pub use request_prep::{ReminderRequestContext, prepare_session_messages};
pub use retry::RetryConfig;
pub use token_estimate::{
    CHARS_PER_TOKEN, anchored_estimate, estimate_conversation_tokens, estimate_message_tokens,
};
pub use turn_ext::TurnExtensions;
pub use turn_loop::{LoopContext, LoopOutcome};
