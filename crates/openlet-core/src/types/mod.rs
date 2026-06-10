//! Core domain types — IO-free, plain data.

pub mod agent;
pub mod event;
pub mod message;
pub mod part;
pub mod permission;
pub mod session;

pub use agent::{AgentId, AgentSpec};
pub use event::{AgentEvent, EventFilter};
pub use message::{Message, MessageId, Role};
pub use part::{Part, PartId};
pub use permission::{AlwaysScope, Decision, PermissionMode, PermissionRequest, PermissionRule};
pub use session::{SessionFilter, SessionId, SessionMeta, SessionStatus};
