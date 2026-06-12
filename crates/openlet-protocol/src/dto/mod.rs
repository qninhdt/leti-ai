//! HTTP DTOs — kept separate from `openlet-core` so domain types stay
//! HTTP-agnostic. Each module is one DTO group.

pub mod agent;
pub mod error;
pub mod event;
pub mod health;
pub mod message;
pub mod model;
pub mod part;
pub mod permission;
pub mod question;
pub mod session;

pub use agent::AgentDto;
pub use error::ErrorDto;
pub use event::{
    AskOptionDto, AttachmentKindDto, DeltaKindDto, EventDto, NotificationLevelDto,
    PermissionDecisionDto, UsageDto,
};
pub use health::HealthDto;
pub use message::{CreateMessageDto, MessageDto, PromptAckDto};
pub use model::ModelDto;
pub use part::PartDto;
pub use permission::{PermissionReplyDto, PermissionReplyKind, PermissionRequestDto};
pub use question::QuestionAnswerDto;
pub use session::{AbortAckDto, CreateSessionDto, SessionDto, SetModeDto};
