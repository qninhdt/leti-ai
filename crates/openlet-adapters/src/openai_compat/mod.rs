//! OpenAI-compat / OpenRouter `ModelProvider` impl.
//!
//! Three-layer split per phase-03 §Architecture:
//!   1. `provider` — HTTP send + cancellation + status mapping
//!   2. `wire`     — `ChatRequest` ↔ OpenAI JSON shape
//!   3. `sse` + `chunk_decoder` — frame extraction + `ChatDelta` decode
//!
//! `pricing` is the static OpenRouter pricing table.

pub mod chunk_decoder;
pub mod prefix_shaping;
pub mod pricing;
pub mod provider;
pub mod sse;
pub(crate) mod stream;
pub mod wire;

pub use prefix_shaping::{apply_request_shaping, detect_quirks};
pub use provider::{DEFAULT_BASE_URL, OpenAiCompatProvider};
