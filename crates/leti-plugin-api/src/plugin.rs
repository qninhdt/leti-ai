use async_trait::async_trait;
use thiserror::Error;

use crate::context::PluginContext;
use crate::manifest::PluginManifest;

/// The single trait every plugin implements.
///
/// `install` registers handlers via `ctx.on_*`. Plugin authors use only
/// this crate.
#[async_trait]
pub trait Plugin: Send + Sync + 'static {
    fn manifest(&self) -> &PluginManifest;

    async fn install(&self, ctx: &mut PluginContext) -> Result<(), PluginError>;

    async fn shutdown(&self) -> Result<(), PluginError> {
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("plugin {id} install failed: {message}")]
    Install { id: String, message: String },

    #[error("plugin {id} core_version_req {req} unsatisfied (have {have})")]
    IncompatibleCoreVersion {
        id: String,
        req: String,
        have: String,
    },

    #[error("plugin config invalid: {0}")]
    InvalidConfig(String),

    #[error("plugin runtime error: {0}")]
    Runtime(String),
}
