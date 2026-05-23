//! Conversation runtime — orchestrator + pure-logic helpers.
//!
//! Phase 3 slice 1 landed the pure helpers (`processor`, `doom_guard`,
//! `cost`). Slice 2 adds the `ConversationRuntime` orchestrator
//! (`conversation`) and the streaming-id bookkeeping (`turn_stream`)
//! that bridges `Processor` to `MemoryStore` + `EventSink`.

pub mod conversation;
pub mod cost;
pub mod doom_guard;
pub mod processor;
pub mod turn_loop;
mod turn_stream;

pub use conversation::{ConversationRuntime, RuntimeConfig, TurnInput, TurnOutcome};
pub use cost::{compute_cost, format_usd};
pub use doom_guard::{DoomVerdict, ToolCallSig};
pub use processor::{Processor, ProcessorEvent, ProcessorPart, ProcessorState};
pub use turn_loop::{LoopContext, LoopOutcome};
