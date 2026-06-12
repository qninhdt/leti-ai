//! Shared helpers for OpenAI-compatible providers.
//!
//! Both `OpenAiProvider` and `OpenRouterProvider` delegate `pricing`,
//! `list_models`, and `capabilities` to the same underlying functions.
//! This module re-exports them under a single namespace so the providers
//! avoid importing from multiple sibling modules.

use openlet_core::adapters::model_provider::{ModelInfo, ModelPricing, ProviderCapabilities};
use openlet_core::error::ProviderError;

use super::prefix_shaping::detect_quirks;
use super::pricing::pricing_for;
use super::transport::HttpTransport;

/// Shared pricing lookup — delegates to the static pricing table.
#[inline]
pub(crate) fn shared_pricing(model: &str) -> Option<ModelPricing> {
    pricing_for(model)
}

/// Shared capabilities lookup — delegates to prefix-shaping quirk detection.
#[inline]
pub(crate) fn shared_capabilities(model: &str) -> ProviderCapabilities {
    detect_quirks(model)
}

/// Shared list_models — delegates to the transport's GET /models endpoint.
pub(crate) async fn shared_list_models(
    transport: &HttpTransport,
) -> Result<Vec<ModelInfo>, ProviderError> {
    transport.list_models().await
}
