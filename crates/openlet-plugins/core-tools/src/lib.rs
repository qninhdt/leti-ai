//! `core-tools` plugin — registers the eight built-in tools (read,
//! list, glob, grep, write, edit, bash, todo) through the public
//! `register_tool` extension point. Closes the amendment §5 dogfood
//! test: if MVP can't ship its own tools through the plugin API, the
//! API is wrong.
//!
//! `bash` needs a `ShellExecutor`. Trait objects can't deserialize
//! through `ctx.config()`, so the executor is injected at plugin
//! construction time — the host (server crate) builds the
//! `LocalShellExecutor` and hands it to `CoreToolsPlugin::new`. Other
//! integrators substitute a different `ShellExecutor` impl (mock,
//! sandboxed, remote) without forking core.

use std::sync::Arc;

use async_trait::async_trait;
use openlet_core::tools::Tool;
use openlet_core::tools::builtins::bash::ShellExecutor;
use openlet_core::tools::builtins::{
    BashTool, EditTool, GlobTool, GrepTool, ListTool, ReadTool, TodoTool, WriteTool,
};
use openlet_plugin_api::manifest::Capability;
use openlet_plugin_api::{Plugin, PluginContext, PluginError, PluginManifest};
use semver::{Version, VersionReq};

/// Erase a typed `Tool` impl into the object-safe handle the plugin
/// API stores. Mirrors `ToolRegistryBuilder::register` in spirit but
/// produces a `ToolHandle` we can hand to `ctx.register_tool`.
fn erase<T>(tool: T) -> openlet_core::tools::ToolHandle
where
    T: Tool + 'static,
{
    Arc::new(tool)
}

pub struct CoreToolsPlugin {
    manifest: PluginManifest,
    shell: Arc<dyn ShellExecutor>,
}

impl CoreToolsPlugin {
    /// `shell` is the bash executor the `bash` tool forwards into.
    /// Pass `LocalShellExecutor` from the adapter crate (production)
    /// or a mock impl (tests). Cloning is cheap — the plugin only
    /// holds the `Arc`; the registered tool clones it once into its
    /// own handle.
    #[must_use]
    pub fn new(shell: Arc<dyn ShellExecutor>) -> Self {
        Self {
            manifest: PluginManifest {
                id: "core-tools".into(),
                name: "Openlet Core Tools".into(),
                version: Version::new(0, 1, 0),
                description:
                    "Ships the eight built-in tools (read, list, glob, grep, write, edit, \
                     bash, todo) through the plugin extension surface."
                        .into(),
                author: Some("Openlet".into()),
                capabilities: vec![Capability::Tool],
                core_version_req: VersionReq::parse(">=0.1.0").expect("static version req"),
                default_priority: 50,
                config_schema: None,
            },
            shell,
        }
    }
}

#[async_trait]
impl Plugin for CoreToolsPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    async fn install(&self, ctx: &mut PluginContext) -> Result<(), PluginError> {
        ctx.register_tool(erase(ReadTool))?;
        ctx.register_tool(erase(ListTool))?;
        ctx.register_tool(erase(GlobTool))?;
        ctx.register_tool(erase(GrepTool))?;
        ctx.register_tool(erase(WriteTool))?;
        ctx.register_tool(erase(EditTool))?;
        ctx.register_tool(erase(BashTool::with_executor(self.shell.clone())))?;
        ctx.register_tool(erase(TodoTool))?;
        Ok(())
    }
}
