//! HTTP routes — one module per feature group. Phase 5 wires the
//! session/message/cancel/agent/permission/event/plugin surface.

pub mod agent;
pub mod cancel;
pub mod diagnostics;
pub mod event;
pub mod health;
pub mod message;
pub mod permission;
pub mod plugin;
pub mod session;
