use std::fmt;

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

/// Provenance-bearing kind of a runtime reminder. A reminder is
/// harness-authored context injected into the model's view of the
/// conversation. The variant is a CLOSED set: trusted provenance exists
/// only because runtime code constructed this enum — never because user
/// text happened to contain `<system-reminder>` tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReminderKind {
    /// Execution mode / constraint the agent must respect this turn.
    ExecutionConstraint,
    /// Subagent / task lifecycle transition the parent should know about.
    TaskState,
    /// A file the session previously observed changed or was deleted.
    WorkspaceDelta,
    /// A background subagent completed and is awaiting parent delivery.
    BackgroundTaskSettled,
    /// Post-compaction recovery notice re-anchoring still-active state.
    CompactionRecovery,
    /// Configured token / cost / turn threshold was reached.
    RuntimeLimit,
    /// An exceptional permission / tool outcome worth surfacing.
    ExceptionalOutcome,
}

/// Durable lifecycle of a compaction request. A failed marker remains an
/// audit record but is excluded from both model projection and the timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum CompactionAttemptState {
    #[default]
    Pending,
    Committed,
    Failed,
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
    /// Image attachment. Bytes live in the `ArtifactStore` keyed by
    /// `artifact_id`; the part carries only the pointer + display
    /// metadata so the on-the-wire JSON stays compact and replay
    /// doesn't drag pixels through the projection layer.
    Image {
        id: PartId,
        artifact_id: String,
        mime: String,
        width: u32,
        height: u32,
    },
    /// PDF (or future document type) attachment. Original bytes are in
    /// the artifact store; `extracted_text` carries the truncated inline
    /// preview that the model sees during projection (full text is
    /// always available via the artifact id).
    Document {
        id: PartId,
        artifact_id: String,
        mime: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extracted_text: Option<String>,
    },
    StepStart {
        id: PartId,
    },
    StepFinish {
        id: PartId,
        reason: String,
    },
    /// A compaction summary produced by the compaction turn. Replaces
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
        /// Used by post-compaction overflow check.
        original_token_count: u32,
    },
    /// Durable compaction boundary. Request wording is injected only by the
    /// explicit compaction projection and never persisted as user text.
    CompactionRequest {
        id: PartId,
        #[serde(default)]
        state: CompactionAttemptState,
        /// Assistant message allocated for this attempt. On failure this lets
        /// replay and the TUI suppress a partial, non-committed summary.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary_message_id: Option<String>,
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
    /// Harness-authored runtime reminder. Rendered to the model at the
    /// projection boundary as `<system-reminder>` user-side content, but
    /// NEVER rendered as a human-authored user bubble in the TUI. Trusted
    /// provenance is established solely by this typed variant existing —
    /// runtime code is the only constructor. `stable_key` identifies the
    /// logical reminder for effective-projection dedupe; `projection_epoch`
    /// lets an active reminder be re-anchored after a compaction without a
    /// superseded copy suppressing its replacement.
    RuntimeReminder {
        id: PartId,
        // Named `reminder_kind` (not `kind`) because the enum's serde
        // discriminator is `tag = "kind"` — an inner `kind` field would
        // collide with the variant tag on the wire.
        reminder_kind: ReminderKind,
        stable_key: String,
        content: String,
        projection_epoch: u32,
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
            | Self::Document { id, .. }
            | Self::StepStart { id, .. }
            | Self::StepFinish { id, .. }
            | Self::Compaction { id, .. }
            | Self::CompactionRequest { id, .. }
            | Self::Plan { id, .. }
            | Self::RuntimeReminder { id, .. } => *id,
        }
    }
}
