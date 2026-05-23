//! Pure incremental processor — `ChatDelta` → assistant message parts + events.
//!
//! Stateless logic on top of `ProcessorState`. No IO. No async. The runtime
//! drives this from the provider's stream and dispatches the resulting
//! writes/events to storage + the bus.
//!
//! Cross-check (`research/cross-check-phase-03.md` §3): args accumulated in
//! a `String` buffer, parsed only on `Finish`. §T: duplicate `(name, index)`
//! triggers a typed `ProviderError::Decode` mid-stream.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::adapters::model_provider::{ChatDelta, FinishReason};
use crate::error::ProviderError;
use crate::types::event::{DeltaKind, Usage};

/// Accumulator for a single in-progress tool call.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PendingToolCall {
    pub call_id: String,
    pub name: String,
    pub args_buf: String,
}

/// State carried between `Processor::step` calls within one assistant turn.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessorState {
    pub current_text: String,
    pub current_reasoning: String,
    pub current_reasoning_signature: Option<String>,
    pub pending_tool_calls: BTreeMap<usize, PendingToolCall>,
    pub usage: Option<Usage>,
    pub finish: Option<FinishReason>,
}

/// Materialized assistant-side part the runtime should persist. The runtime
/// owns id assignment + storage; the processor only emits the payload.
#[derive(Debug, Clone, PartialEq)]
pub enum ProcessorPart {
    Text {
        text: String,
    },
    Reasoning {
        text: String,
        signature: Option<String>,
    },
    ToolCall {
        call_id: String,
        name: String,
        args: serde_json::Value,
    },
    StepFinish {
        reason: String,
        usage: Option<Usage>,
    },
}

/// Event the runtime should publish on the bus. Persistence flag (durable
/// vs transient per amendment §G) is decided at the runtime layer.
#[derive(Debug, Clone, PartialEq)]
pub enum ProcessorEvent {
    /// Streaming delta — transient. `delta` is the new fragment ONLY.
    PartDelta { kind: DeltaKind, delta: String },
}

/// One step's output: parts to persist + events to publish + next state.
#[derive(Debug, Clone)]
pub struct StepOutcome {
    pub parts: Vec<ProcessorPart>,
    pub events: Vec<ProcessorEvent>,
    pub next: ProcessorState,
}

/// Pure processor — `step` is a total function: same `(state, delta)` ⇒
/// same outcome.
#[derive(Debug, Default)]
pub struct Processor;

impl Processor {
    /// Apply one `ChatDelta`. Returns parts/events to persist+publish and
    /// the next state. Errors short-circuit the turn (the runtime should
    /// stop draining the stream and finalize with `reason=error`).
    pub fn step(
        mut state: ProcessorState,
        delta: ChatDelta,
    ) -> Result<StepOutcome, ProviderError> {
        let mut parts: Vec<ProcessorPart> = Vec::new();
        let mut events: Vec<ProcessorEvent> = Vec::new();

        match delta {
            ChatDelta::Role => {
                // No-op for the storage layer; some providers send Role once.
            }
            ChatDelta::Content { text } => {
                if !text.is_empty() {
                    state.current_text.push_str(&text);
                    events.push(ProcessorEvent::PartDelta {
                        kind: DeltaKind::Text,
                        delta: text,
                    });
                }
            }
            ChatDelta::Reasoning { text, signature } => {
                if signature.is_some() {
                    state.current_reasoning_signature = signature;
                }
                if !text.is_empty() {
                    state.current_reasoning.push_str(&text);
                    events.push(ProcessorEvent::PartDelta {
                        kind: DeltaKind::Reasoning,
                        delta: text,
                    });
                }
            }
            ChatDelta::ToolCallStart {
                call_id,
                name,
                index,
            } => {
                if let Some(existing) = state.pending_tool_calls.get(&index) {
                    let name_collision =
                        !existing.name.is_empty() && !name.is_empty() && existing.name != name;
                    let already_streaming = !existing.args_buf.is_empty();
                    if name_collision || already_streaming {
                        return Err(ProviderError::Decode(format!(
                            "duplicate tool_call index {index} (existing call_id={}, \
                             incoming call_id={call_id})",
                            existing.call_id
                        )));
                    }
                }
                state.pending_tool_calls.insert(
                    index,
                    PendingToolCall {
                        call_id,
                        name,
                        args_buf: String::new(),
                    },
                );
            }
            ChatDelta::ToolCallArgsDelta { index, args_chunk } => {
                let entry = state.pending_tool_calls.entry(index).or_default();
                entry.args_buf.push_str(&args_chunk);
                events.push(ProcessorEvent::PartDelta {
                    kind: DeltaKind::ToolArgs,
                    delta: args_chunk,
                });
            }
            ChatDelta::Finish { reason, usage } => {
                state.usage = usage.or(state.usage);
                state.finish = Some(reason);
                flush_into_parts(&mut state, &mut parts)?;
            }
        }

        Ok(StepOutcome {
            parts,
            events,
            next: state,
        })
    }
}

/// Drain the accumulator into terminal parts. Called on `Finish`. Validates
/// every pending tool call's `args_buf` parses as JSON; an empty buffer is
/// treated as `{}` (some providers omit args for zero-param tools).
fn flush_into_parts(
    state: &mut ProcessorState,
    parts: &mut Vec<ProcessorPart>,
) -> Result<(), ProviderError> {
    if !state.current_reasoning.is_empty() {
        parts.push(ProcessorPart::Reasoning {
            text: std::mem::take(&mut state.current_reasoning),
            signature: state.current_reasoning_signature.take(),
        });
    }
    if !state.current_text.is_empty() {
        parts.push(ProcessorPart::Text {
            text: std::mem::take(&mut state.current_text),
        });
    }
    let pending = std::mem::take(&mut state.pending_tool_calls);
    for (idx, call) in pending {
        if call.name.is_empty() {
            return Err(ProviderError::Decode(format!(
                "tool_call index {idx} finished without a name"
            )));
        }
        let args_str = if call.args_buf.is_empty() {
            "{}".to_string()
        } else {
            call.args_buf
        };
        let args: serde_json::Value = serde_json::from_str(&args_str).map_err(|e| {
            ProviderError::Decode(format!(
                "tool_call args invalid JSON (call_id={}, name={}): {e}",
                call.call_id, call.name
            ))
        })?;
        parts.push(ProcessorPart::ToolCall {
            call_id: call.call_id,
            name: call.name,
            args,
        });
    }
    if let Some(reason) = state.finish {
        parts.push(ProcessorPart::StepFinish {
            reason: finish_reason_label(reason).to_string(),
            usage: state.usage.clone(),
        });
    }
    Ok(())
}

fn finish_reason_label(reason: FinishReason) -> &'static str {
    match reason {
        FinishReason::EndTurn => "end_turn",
        FinishReason::ToolUse => "tool_use",
        FinishReason::MaxTokens => "max_tokens",
        FinishReason::Length => "length",
        FinishReason::ContentFilter => "content_filter",
        FinishReason::Error => "error",
        FinishReason::Cancelled => "cancelled",
    }
}
