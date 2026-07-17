//! Leti core — domain types, adapter traits, config, runtime stubs.
//!
//! This crate is IO-free: it defines the contracts later phases plug into.
//! Adapter trait surface is locked here; implementations live in
//! `leti-adapters`.
//!
//! # Host boundary
//!
//! Core is host-context-blind by design. It receives an `AgentId`
//! and port implementations. Each agent owns exactly one workspace; routing
//! belongs in a plugin that wraps core.
//!
//! Ownership, routing, and request policy live at the host boundary. Plugins
//! map their external context to an `AgentId` before dispatching into
//! core.
//!
//! The wire-format `Role::User` literal in [`types::message`] refers to
//! the *OpenAI/OpenRouter chat-message role* (a protocol string), not
//! to a human participant — it stays as-is regardless of the host model.

#![allow(clippy::module_inception)]

pub mod adapters;
pub mod agent;
pub mod config;
pub mod dispatch;
pub mod error;
pub mod hooks;
pub mod permission;
pub mod projection;
pub mod runtime;
pub mod tools;
pub mod types;

pub use error::{CoreError, FailureClass};
