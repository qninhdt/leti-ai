//! Shared test helpers for `leti-adapters` integration tests.
//!
//! Exposes:
//! - [`sqlite_helper::make_pool`] — fresh `:memory:` SQLite pool with
//!   migrations applied. Re-export of [`leti_adapters::sqlite::open_in_memory`]
//!   under a more obvious name.
//! - [`tempdir_workspace::WorkspaceFixture`] — a `TempDir` + workspace
//!   root path; constructors `empty()` and `with_files(...)`.

#![allow(dead_code)]

pub mod mem_fs;
pub mod sqlite_helper;
pub mod tempdir_workspace;
pub mod tool_ctx_harness;
pub mod wiremock_helpers;
