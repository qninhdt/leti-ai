//! Local adapter implementations.
//!
//! Each module hosts one of the six adapter trait impls:
//! `openai_compat`, `sqlite`, `localfs`, `localshell`, `bus`, `config_perm`.
//! Provider modules add `anthropic`, `gemini`, and the `multi_provider` router.

pub mod anthropic;
pub mod bus;
pub mod config_perm;
pub mod gemini;
pub mod localfs;
pub mod localshell;
pub(crate) mod model_match;
pub mod multi_provider;
pub mod openai_compat;
pub mod sqlite;
pub(crate) mod stub_provider;
