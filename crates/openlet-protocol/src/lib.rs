//! Openlet HTTP DTOs — utoipa-derived schemas shared by server + future SDK.
//!
//! Kept separate from `openlet-core` so domain types stay HTTP-agnostic.

pub mod dto;

pub use dto::{
    AbortAckDto, AgentDto, CreateMessageDto, CreateSessionDto, DeltaKindDto, ErrorDto, EventDto,
    HealthDto, MessageDto, PartDto, PermissionReplyDto, PermissionReplyKind, PermissionRequestDto,
    PromptAckDto, SessionDto, SetModeDto, UsageDto,
};
