//! Conversation runtime — orchestrator + pure-logic helpers.
//!
//! Phase 3 slice 1 landed the pure helpers (`processor`, `doom_guard`,
//! `cost`). Slice 2 adds the `ConversationRuntime` orchestrator
//! (`conversation`) and the streaming-id bookkeeping (`turn_stream`)
//! that bridges `Processor` to `MemoryStore` + `EventSink`.

pub mod agent_allowlist;
pub mod attachments;
pub mod compaction;
pub mod conversation;
pub mod cost;
pub mod doom_guard;
pub mod processor;
pub mod prompt;
pub mod question_registry;
pub mod token_estimate;
pub mod turn_loop;
mod turn_stream;

pub use compaction::{
    COMPACTION_REQUEST, CompactDecision, PRESERVE_RECENT, build_compaction_projection,
    should_compact, superseded_messages,
};
pub use conversation::{ConversationRuntime, RuntimeConfig, TurnInput, TurnOutcome};
pub use cost::{compute_cost, format_usd};
pub use doom_guard::{DoomVerdict, ToolCallSig};
pub use processor::{Processor, ProcessorEvent, ProcessorPart, ProcessorState};
pub use prompt::{compose_system_prompt, select_provider_prompt};
pub use question_registry::{CancelReason, QuestionId, QuestionRegistry, ResolveError};
pub use token_estimate::{
    CHARS_PER_TOKEN, anchored_estimate, estimate_conversation_tokens, estimate_message_tokens,
};
pub use turn_loop::{LoopContext, LoopOutcome};
