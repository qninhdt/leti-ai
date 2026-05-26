//! Shared test helpers for `openlet-adapters` integration tests.
//!
//! Exposes:
//! - [`sqlite_helper::make_pool`] — fresh `:memory:` SQLite pool with
//!   migrations applied. Re-export of [`openlet_adapters::sqlite::open_in_memory`]
//!   under a more obvious name.
//! - [`tempdir_workspace::WorkspaceFixture`] — a `TempDir` + workspace
//!   root path; constructors `empty()` and `with_files(...)`.

#![allow(dead_code)]

pub mod sqlite_helper;
pub mod tempdir_workspace;
pub mod wiremock_helpers;
