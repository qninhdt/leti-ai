use std::sync::Arc;

use serde::de::DeserializeOwned;

use crate::manifest::PluginManifest;
use crate::plugin::PluginError;

/// Registration API exposed to plugins during `install`.
///
/// Phase 1 ships a minimal context — `manifest()` + `config()`. Hook
/// registration methods (`on_chat_params`, `before_tool_call`, …) land
/// alongside the runtime that consumes them in Phase 3+.
pub struct PluginContext {
    manifest: PluginManifest,
    raw_config: serde_json::Value,
    core_api: Arc<dyn CoreApi>,
}

impl PluginContext {
    #[must_use]
    pub fn new(
        manifest: PluginManifest,
        raw_config: serde_json::Value,
        core_api: Arc<dyn CoreApi>,
    ) -> Self {
        Self {
            manifest,
            raw_config,
            core_api,
        }
    }

    #[must_use]
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// Deserializes the per-plugin config block.
    pub fn config<T: DeserializeOwned>(&self) -> Result<T, PluginError> {
        serde_json::from_value(self.raw_config.clone())
            .map_err(|e| PluginError::InvalidConfig(e.to_string()))
    }

    #[must_use]
    pub fn core(&self) -> Arc<dyn CoreApi> {
        Arc::clone(&self.core_api)
    }
}

/// Typed back-channel into core. Phase 1 keeps the trait empty; later
/// phases add `current_session_meta`, `emit_event`, `record_cost`, …
pub trait CoreApi: Send + Sync + 'static {}
