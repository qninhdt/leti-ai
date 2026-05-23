//! Local adapter implementations (Phase 1: stubs only).
//!
//! Each module hosts one of the six adapter trait impls:
//! `openai_compat`, `sqlite`, `localfs`, `localshell`, `bus`, `config_perm`.

pub mod bus;
pub mod config_perm;
pub mod localfs;
pub mod localshell;
pub mod openai_compat;
pub mod sqlite;
