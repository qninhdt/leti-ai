//! Persistence helpers — append (Message|Part) + publish the matching
//! `*Created` event in one step.
//!
//! Every `MemoryStore::append_*` call in the runtime is paired with an
//! `EventSink::publish(Persistence::Durable, AgentEvent::*Created)`. The
//! pair is identical at every site (six call sites for messages, five for
//! parts), so it lives here as a single helper.

use std::sync::Arc;

use chrono::Utc;

use crate::adapters::event_sink::{EventSink, Persistence};
use crate::adapters::memory_store::MemoryStore;
use crate::error::CoreError;
use crate::types::event::AgentEvent;
use crate::types::message::{Message, MessageId, Role};
use crate::types::part::{Part, PartId};
use crate::types::session::SessionId;

/// Append a fresh `Message` to the store and publish the matching
/// `MessageCreated` event durably. Returns the storage-assigned id.
pub(crate) async fn append_message_with_event(
    memory: &Arc<dyn MemoryStore>,
    events: &Arc<dyn EventSink>,
    session_id: SessionId,
    role: Role,
) -> Result<MessageId, CoreError> {
    let msg = Message {
        id: MessageId::new(),
        session_id,
        role,
        created_at: Utc::now(),
    };
    let mid = memory.append_message(session_id, msg).await?;
    events
        .publish(
            AgentEvent::MessageCreated {
                session_id,
                message_id: mid,
                at: Utc::now(),
            },
            Persistence::Durable,
        )
        .await?;
    Ok(mid)
}

/// Append a `Part` to a message and publish the matching `PartCreated`
/// event durably. Returns the part's id (echoed from `part.id()`).
pub(crate) async fn append_part_with_event(
    memory: &Arc<dyn MemoryStore>,
    events: &Arc<dyn EventSink>,
    session_id: SessionId,
    message_id: MessageId,
    part: Part,
) -> Result<PartId, CoreError> {
    let part_id = part.id();
    memory.append_part(message_id, part).await?;
    events
        .publish(
            AgentEvent::PartCreated {
                session_id,
                message_id,
                part_id,
                at: Utc::now(),
            },
            Persistence::Durable,
        )
        .await?;
    Ok(part_id)
}
