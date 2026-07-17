//! OpenRouter `ModelProvider` — extends the base `openai` adapter.
//!
//! OpenRouter speaks the OpenAI `chat/completions` dialect but adds
//! vendor features the generic adapter ignores: app attribution
//! (`HTTP-Referer` / `X-Title`), provider routing preferences, and a
//! `models` fallback array. This adapter reuses the base transport,
//! wire serialization, prefix-shaping, and pricing — it only enriches
//! the outbound request.
//!
//! Use this adapter when talking to OpenRouter. For any other
//! OpenAI-compatible gateway, use [`crate::openai::OpenAiProvider`].

mod config;
mod provider;

pub use config::{OpenRouterConfig, ProviderRouting};
pub use provider::{DEFAULT_BASE_URL, OpenRouterProvider};
