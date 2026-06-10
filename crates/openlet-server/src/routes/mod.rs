//! HTTP routes — one module per feature group. Wires the
//! session/message/cancel/agent/permission/event/plugin surface.

pub mod agent;
pub mod attachments;
pub mod cancel;
pub mod diagnostics;
pub mod event;
pub mod health;
pub mod message;
pub mod model;
pub mod permission;
pub mod plugin;
pub mod question;
pub mod session;
