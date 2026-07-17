//! Leti HTTP DTOs — utoipa-derived schemas shared by server + future SDK.
//!
//! Kept separate from `leti-core` so domain types stay HTTP-agnostic.

pub mod dto;

pub use dto::{
    AbortAckDto, AgentDto, AskOptionDto, AttachmentKindDto, BackgroundTaskAckDto, CompactAckDto,
    ContinueSubagentDto, CreateMessageDto, CreateSessionDto, DeltaKindDto, ErrorDto, EventDto,
    HealthDto, MessageDto, ModelDto, NotificationLevelDto, PartDto, PermissionDecisionDto,
    PermissionReplyDto, PermissionReplyKind, PermissionRequestDto, PromptAckDto, QuestionAnswerDto,
    SessionDto, SetModeDto, SubagentControlAckDto, SubagentExecutionDto, UsageDto,
};
