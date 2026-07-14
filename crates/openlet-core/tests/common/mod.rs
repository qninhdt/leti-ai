//! Shared test mocks for `openlet-core` integration tests.
//!
//! Exposes:
//! - [`mock_provider::ScriptedProvider`] — scripted `ChatDelta` queue with
//!   cancellation peek between deltas.
//! - [`mock_event_sink::RecordingEventSink`] — captures every `publish` for
//!   later drain via `take()`.
//! - [`mock_permission::AllowAll`] / [`mock_permission::DenyAll`] /
//!   [`mock_permission::ScriptedPermission`] — three deterministic gates.
//! - [`mock_tool`] — a registry builder with `noop`, `failing`, `slow`,
//!   `panicking` helper tools.
//! - [`mock_artifact::MemArtifactStore`] — an in-memory `ArtifactStore`.
//! - [`mock_memory::MockMemoryStore`] — minimal in-memory message store.
//!
//! Each test file picks the helpers it needs via `mod common;` then
//! `use common::mock_provider::ScriptedProvider;` etc.

#![allow(dead_code)]

pub mod mock_artifact;
pub mod mock_event_sink;
pub mod mock_memory;
pub mod mock_permission;
pub mod mock_provider;
pub mod mock_tool;
pub mod runtime;
pub mod tool_ctx;
