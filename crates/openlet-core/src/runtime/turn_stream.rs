//! Per-turn streaming-part bookkeeping for `ConversationRuntime`.
//!
//! Bridges `Processor` (pure, no IDs) to the `MemoryStore` + `EventSink`
//! pair (which need IDs and durable/transient routing). Pre-allocates one
//! `PartId` per text/reasoning stream, persists empty shells on first
//! delta (`PartCreated` durable), broadcasts each chunk (`PartDelta`
//! transient), and replaces with the finalized body when the processor
//! flushes terminal parts (`PartUpdated` durable).
//!
//! Tool-call argument deltas are not pushed onto the bus in slice 2 —
//! the processor reports them without `(part_id, call_id)` mapping, so
//! we only persist the final `ToolCall` part on `Finish`.

use std::sync::Arc;

use crate::adapters::event_sink::{EventSink, Persistence};
use crate::adapters::memory_store::MemoryStore;
use crate::error::CoreError;
use crate::runtime::persist::append_part_with_event;
use crate::runtime::processor::{ProcessorEvent, ProcessorPart};
use crate::types::event::{AgentEvent, DeltaKind};
use crate::types::message::MessageId;
use crate::types::part::{Part, PartId};
use crate::types::session::SessionId;

/// Per-turn state: which streaming parts have been pre-allocated.
#[derive(Debug, Default)]
pub(crate) struct StreamingPartTracker {
    pub text_part: Option<PartId>,
    pub reasoning_part: Option<PartId>,
}

impl StreamingPartTracker {
    /// Handle one `ProcessorEvent`. Allocates streaming part IDs lazily
    /// and routes to `EventSink` with the correct persistence tier.
    pub(crate) async fn handle_event(
        &mut self,
        memory: &Arc<dyn MemoryStore>,
        events: &Arc<dyn EventSink>,
        session_id: SessionId,
        message_id: MessageId,
        evt: ProcessorEvent,
    ) -> Result<(), CoreError> {
        match evt {
            ProcessorEvent::PartDelta { kind, delta } => {
                let part_id = match kind {
                    DeltaKind::Text => {
                        self.ensure_streaming_part(
                            memory,
                            events,
                            session_id,
                            message_id,
                            DeltaKind::Text,
                        )
                        .await?
                    }
                    DeltaKind::Reasoning => {
                        self.ensure_streaming_part(
                            memory,
                            events,
                            session_id,
                            message_id,
                            DeltaKind::Reasoning,
                        )
                        .await?
                    }
                    // Slice-2: tool args have no part_id mapping yet.
                    DeltaKind::ToolArgs => return Ok(()),
                };
                events
                    .publish(
                        AgentEvent::PartDelta {
                            session_id,
                            message_id,
                            part_id,
                            delta_kind: kind,
                            delta,
                        },
                        Persistence::Transient,
                    )
                    .await?;
            }
        }
        Ok(())
    }

    /// Handle one terminal `ProcessorPart` flushed at `Finish`. For text
    /// and reasoning, replaces the pre-allocated empty shell with the
    /// final body; for tool-call and step-finish, allocates a fresh ID
    /// and persists once.
    pub(crate) async fn handle_part(
        &mut self,
        memory: &Arc<dyn MemoryStore>,
        events: &Arc<dyn EventSink>,
        session_id: SessionId,
        message_id: MessageId,
        part: ProcessorPart,
        cost_decimal_str: Option<String>,
    ) -> Result<(), CoreError> {
        match part {
            ProcessorPart::Text { text } => {
                let part_id = self
                    .ensure_streaming_part(memory, events, session_id, message_id, DeltaKind::Text)
                    .await?;
                memory
                    .upsert_part(message_id, part_id, Part::Text { id: part_id, text })
                    .await?;
                events
                    .publish(
                        AgentEvent::PartUpdated {
                            session_id,
                            message_id,
                            part_id,
                        },
                        Persistence::Durable,
                    )
                    .await?;
            }
            ProcessorPart::Reasoning { text, signature: _ } => {
                let part_id = self
                    .ensure_streaming_part(
                        memory,
                        events,
                        session_id,
                        message_id,
                        DeltaKind::Reasoning,
                    )
                    .await?;
                memory
                    .upsert_part(message_id, part_id, Part::Reasoning { id: part_id, text })
                    .await?;
                events
                    .publish(
                        AgentEvent::PartUpdated {
                            session_id,
                            message_id,
                            part_id,
                        },
                        Persistence::Durable,
                    )
                    .await?;
            }
            ProcessorPart::ToolCall {
                call_id,
                name,
                args,
            } => {
                let part_id = PartId::new();
                append_part_with_event(
                    memory,
                    events,
                    session_id,
                    message_id,
                    Part::ToolCall {
                        id: part_id,
                        call_id,
                        name,
                        args,
                    },
                )
                .await?;
            }
            ProcessorPart::StepFinish { reason, usage } => {
                let part_id = PartId::new();
                append_part_with_event(
                    memory,
                    events,
                    session_id,
                    message_id,
                    Part::StepFinish {
                        id: part_id,
                        reason: reason.clone(),
                    },
                )
                .await?;
                events
                    .publish(
                        AgentEvent::StepFinished {
                            session_id,
                            message_id,
                            reason,
                            usage,
                            cost_decimal_str,
                        },
                        Persistence::Durable,
                    )
                    .await?;
            }
        }
        Ok(())
    }

    /// Allocate (lazily) and persist an empty streaming-part shell for
    /// either text or reasoning deltas, returning its id. Subsequent calls
    /// for the same `kind` return the cached id without re-publishing.
    /// Panics on `DeltaKind::ToolArgs` — caller must filter that branch.
    async fn ensure_streaming_part(
        &mut self,
        memory: &Arc<dyn MemoryStore>,
        events: &Arc<dyn EventSink>,
        session_id: SessionId,
        message_id: MessageId,
        kind: DeltaKind,
    ) -> Result<PartId, CoreError> {
        let slot = match kind {
            DeltaKind::Text => &mut self.text_part,
            DeltaKind::Reasoning => &mut self.reasoning_part,
            DeltaKind::ToolArgs => unreachable!(
                "ensure_streaming_part is only called for Text/Reasoning; tool args have no part shell"
            ),
        };
        if let Some(id) = *slot {
            return Ok(id);
        }
        let id = PartId::new();
        let part = match kind {
            DeltaKind::Text => Part::Text {
                id,
                text: String::new(),
            },
            DeltaKind::Reasoning => Part::Reasoning {
                id,
                text: String::new(),
            },
            DeltaKind::ToolArgs => unreachable!(),
        };
        append_part_with_event(memory, events, session_id, message_id, part).await?;
        *slot = Some(id);
        Ok(id)
    }
}
