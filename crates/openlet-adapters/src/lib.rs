//! Local adapter implementations.
//!
//! Each module hosts one of the six adapter trait impls:
//! `openai`, `sqlite`, `localfs`, `localshell`, `bus`, `config_perm`.
//! `openrouter` extends `openai` with OpenRouter-specific request
//! enrichment (attribution headers, provider routing, model fallback).

pub mod bus;
pub mod config_perm;
pub mod localfs;
pub mod localshell;
pub mod openai;
pub mod openrouter;
pub mod sqlite;
