//! Compile-time plugin registry.
//!
//! Phase 1 ships an empty registry. Plugins land in later phases via
//! `register()` extensions to `all_plugins()`.

use std::sync::Arc;

use openlet_plugin_api::Plugin;

/// Returns the compile-time list of plugins shipped in this build.
///
/// Empty in Phase 1. Phase 4 introduces `core-tools` + `quota`; Phase 7
/// introduces `core-agents`; Phase 8 introduces `audit-log`.
#[must_use]
pub fn all_plugins() -> Vec<Arc<dyn Plugin>> {
    Vec::new()
}

/// Registry of resolved plugin handles + sorted hook chains.
///
/// Built once at server boot from the result of `all_plugins()` after
/// applying the operator's enabled/disabled config.
#[derive(Default)]
pub struct PluginRegistry {
    plugins: Vec<Arc<dyn Plugin>>,
}

impl PluginRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    pub fn push(&mut self, plugin: Arc<dyn Plugin>) {
        self.plugins.push(plugin);
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn Plugin>> {
        self.plugins.iter()
    }
}
