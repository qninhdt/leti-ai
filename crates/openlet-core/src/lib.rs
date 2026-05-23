//! Openlet core — domain types, adapter traits, config, runtime stubs.
//!
//! This crate is IO-free: it defines the contracts later phases plug into.
//! Adapter trait surface is locked here; implementations live in
//! `openlet-adapters`.

#![allow(clippy::module_inception)]

pub mod adapters;
pub mod config;
pub mod error;
pub mod projection;
pub mod types;

pub use error::{CoreError, FailureClass};
