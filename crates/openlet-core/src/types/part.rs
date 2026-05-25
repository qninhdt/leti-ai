use std::fmt;

use bytes::Bytes;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Strongly-typed part identifier (UUIDv4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PartId(pub Uuid);

impl PartId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for PartId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for PartId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One slice of a message — text token, reasoning chunk, tool call, or
/// tool result. Streaming turns produce many parts per message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Part {
    Text {
        id: PartId,
        text: String,
    },
    Reasoning {
        id: PartId,
        text: String,
    },
    ToolCall {
        id: PartId,
        call_id: String,
        name: String,
        args: serde_json::Value,
    },
    ToolResult {
        id: PartId,
        call_id: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Image {
        id: PartId,
        mime: String,
        #[serde(with = "bytes_serde")]
        bytes: Bytes,
    },
    StepStart {
        id: PartId,
    },
    StepFinish {
        id: PartId,
        reason: String,
    },
    /// A compaction summary produced by phase-07 compaction turn. Replaces
    /// the listed `compacted_message_ids` during projection. Persisted as a
    /// regular Part on the assistant message produced by the compaction
    /// turn — projection substitutes the summary in place of the listed
    /// messages so the LLM sees a compact narrative instead of raw history.
    Compaction {
        id: PartId,
        summary: String,
        /// Message IDs (UUID strings) that this summary supersedes. UUIDs
        /// kept as strings so the JSON column round-trips without sqlx
        /// dragging `Uuid` features into adapter-side Part decoding.
        compacted_message_ids: Vec<String>,
        /// Estimated token count of the messages this summary replaced.
        /// Used by post-compaction overflow check (amendment §P).
        original_token_count: u32,
    },
    /// Frozen plan text emitted by `ExitPlanMode`. Persisted on the
    /// `tool` message that holds the call's result so subsequent turns
    /// (and replay) see the plan verbatim. Projection currently ignores
    /// this part (the tool result already carries the plan to the
    /// model); the durable copy exists for audit / TUI rendering.
    Plan {
        id: PartId,
        plan: String,
    },
}

impl Part {
    #[must_use]
    pub fn id(&self) -> PartId {
        match self {
            Self::Text { id, .. }
            | Self::Reasoning { id, .. }
            | Self::ToolCall { id, .. }
            | Self::ToolResult { id, .. }
            | Self::Image { id, .. }
            | Self::StepStart { id, .. }
            | Self::StepFinish { id, .. }
            | Self::Compaction { id, .. }
            | Self::Plan { id, .. } => *id,
        }
    }
}

mod bytes_serde {
    use bytes::Bytes;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(b: &Bytes, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(b)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Bytes, D::Error> {
        let v: Vec<u8> = Vec::deserialize(d)?;
        Ok(Bytes::from(v))
    }
}
