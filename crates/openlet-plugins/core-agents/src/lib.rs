//! `core-agents` plugin — ships the general assistant + indexer reference
//! agent. Per amendment §6 (Phase 7), built-in agents register through the
//! plugin surface so external Cloud agents can extend or replace them
//! without forking core.

mod general;
mod indexer;

use async_trait::async_trait;
use openlet_plugin_api::manifest::Capability;
use openlet_plugin_api::{Plugin, PluginContext, PluginError, PluginManifest};
use semver::{Version, VersionReq};

pub use general::{GENERAL_CACHEABLE, general_agent};
pub use indexer::indexer_agent;

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
            manifest: PluginManifest {
                id: "core-agents".into(),
                name: "Openlet Core Agents".into(),
                version: Version::new(0, 1, 0),
                description: "Ships the general assistant + indexer reference agents.".into(),
                author: Some("Openlet".into()),
                capabilities: vec![Capability::Agent],
                core_version_req: VersionReq::parse(">=0.1.0").expect("static version req"),
                default_priority: 50,
                config_schema: None,
            },
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
        Ok(())
    }
}
