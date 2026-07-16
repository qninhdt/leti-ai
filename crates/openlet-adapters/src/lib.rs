//! Local adapter implementations.
//!
//! Each module hosts one of the six adapter trait impls:
//! `openai`, `sqlite`, `localfs`, `localshell`, `bus`, `config_perm`.
//! `openrouter` extends `openai` with OpenRouter-specific request
//! enrichment (attribution headers, provider routing, model fallback).

pub mod bus;
pub mod cloudfs;
pub mod config_perm;
pub mod emushell;
pub mod localfs;
pub mod localshell;
pub mod openai;
pub mod openrouter;
pub mod pyexec;
pub mod sqlite;
pub(crate) mod util;
pub mod webfetch;

pub use webfetch::ReqwestWebFetcher;
