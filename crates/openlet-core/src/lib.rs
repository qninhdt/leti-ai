//! Openlet core — domain types, adapter traits, config, runtime stubs.
//!
//! This crate is IO-free: it defines the contracts later phases plug into.
//! Adapter trait surface is locked here; implementations live in
//! `openlet-adapters`.
//!
//! # Auth model
//!
//! **Core is auth-blind by design.** The principal it knows about is
//! [`types::agent::AgentId`] — a UUIDv4 newtype. Each agent owns
//! exactly one workspace; multi-workspace routing belongs in a plugin
//! that wraps core.
//!
//! Auth, JWT validation, user accounts, service accounts, and
//! multi-tenant routing live in **plugins** — typically the cloud
//! deployment's middleware layer. A cloud plugin's job is to map the
//! incoming `(user | service_account)` principal to an `AgentId`
//! before dispatching into core. Core itself never sees user/SA
//! concepts — that polymorphism stays at the boundary.
//!
//! The wire-format `Role::User` literal in [`types::message`] refers to
//! the *OpenAI/OpenRouter chat-message role* (a protocol string), not
//! to a human user — it stays as-is regardless of the auth model.

#![allow(clippy::module_inception)]

pub mod adapters;
pub mod agent;
pub mod config;
pub mod error;
pub mod permission;
pub mod projection;
pub mod runtime;
pub mod tools;
pub mod types;

pub use error::{CoreError, FailureClass};
