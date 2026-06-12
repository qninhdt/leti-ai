//! Base OpenAI-compatible `ModelProvider` impl. Targets any
//! `POST /v1/chat/completions` endpoint speaking the OpenAI dialect.
//! The `openrouter` adapter reuses this transport + wire layer and adds
//! OpenRouter-specific request enrichment.
//!
//! Layered split:
//!   1. `transport` — shared HTTP send, cancellation, status mapping, models parse
//!   2. `provider`  — `ModelProvider` impl + request assembly
//!   3. `wire`      — `ChatRequest` ↔ OpenAI JSON shape
//!   4. `sse` + `chunk_decoder` — frame extraction + `ChatDelta` decode
//!
//! `pricing` is the static model pricing table.

pub mod chunk_decoder;
pub mod prefix_shaping;
pub mod pricing;
pub mod provider;
pub(crate) mod shared_provider;
pub mod sse;
pub(crate) mod stream;
pub mod transport;
pub mod wire;

pub use prefix_shaping::{apply_request_shaping, detect_quirks};
pub use provider::{DEFAULT_BASE_URL, OpenAiProvider};
