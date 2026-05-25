//! OpenAI-compat / OpenRouter `ModelProvider` impl.
//!
//! Three-layer split per phase-03 §Architecture:
//!   1. `provider` — HTTP send + cancellation + status mapping
//!   2. `wire`     — `ChatRequest` ↔ OpenAI JSON shape
//!   3. `sse` + `chunk_decoder` — frame extraction + `ChatDelta` decode
//!
//! `pricing` is the static OpenRouter pricing table.

pub mod capabilities;
pub mod chunk_decoder;
pub mod pricing;
pub mod provider;
pub mod sse;
pub mod wire;

pub use provider::{DEFAULT_BASE_URL, OpenAiCompatProvider};
