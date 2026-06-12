//! `core-agents` plugin — ships the general assistant, plan-mode, and
//! indexer agents. Built-in agents register through the
//! plugin surface so external Cloud agents can extend or replace them
//! without forking core.

mod builder;
mod general;
mod indexer;
mod plan;

use async_trait::async_trait;
use openlet_plugin_api::manifest::Capability;
use openlet_plugin_api::{Plugin, PluginContext, PluginError, PluginManifest};
use semver::Version;

pub use general::{GENERAL_CACHEABLE, general_agent};
pub use indexer::indexer_agent;
pub use plan::{PLAN_CACHEABLE, plan_agent};

/// `core-agents` plugin entry point.
pub struct CoreAgentsPlugin {
    manifest: PluginManifest,
}

impl Default for CoreAgentsPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl CoreAgentsPlugin {
    #[must_use]
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest::builder("core-agents", "Openlet Core Agents")
                .version(Version::new(0, 1, 0))
                .description("Ships the general assistant + indexer + plan-mode agents.")
                .author("Openlet")
                .capabilities(vec![Capability::Agent])
                .default_priority(50)
                .build(),
        }
    }
}

#[async_trait]
impl Plugin for CoreAgentsPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    async fn install(&self, ctx: &mut PluginContext) -> Result<(), PluginError> {
        ctx.register_agent(general_agent())?;
        ctx.register_agent(indexer_agent())?;
        ctx.register_agent(plan_agent())?;
        Ok(())
    }
}
