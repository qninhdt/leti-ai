//! `core-tools` plugin — registers the built-in tools (read, list,
//! glob, grep, write, edit, bash, todo, ask_user, enter_plan_mode,
//! exit_plan_mode, subagent_task, task_status) through the public
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
use openlet_core::adapters::memory_store::MemoryStore;
use openlet_core::runtime::subagent::TaskRegistry;
use openlet_core::tools::Tool;
use openlet_core::tools::builtins::bash::ShellExecutor;
use openlet_core::tools::builtins::subagent_task::SubagentSpawner;
use openlet_core::tools::builtins::{
    AskUserTool, BashTool, EditTool, EnterPlanModeTool, ExitPlanModeTool, GlobTool, GrepTool,
    ListTool, ReadTool, SubagentTaskTool, TaskStatusTool, TodoTool, WriteTool,
};
use openlet_plugin_api::manifest::Capability;
use openlet_plugin_api::{Plugin, PluginContext, PluginError, PluginManifest};
use semver::Version;

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
    memory: Arc<dyn MemoryStore>,
    task_registry: Arc<TaskRegistry>,
    spawner: Arc<dyn SubagentSpawner>,
}

impl CoreToolsPlugin {
    /// `shell` is the bash executor the `bash` tool forwards into.
    /// `memory` is the session store that `enter_plan_mode` /
    /// `exit_plan_mode` mutate to flip the active agent profile.
    /// `task_registry` + `spawner` are required by `subagent_task` /
    /// `task_status`. The host (server crate) builds them once at boot
    /// and threads the same instances into both `AppState` and this
    /// plugin so tool-side bookkeeping matches route-side cancellation.
    /// All are passed in so production wiring (server crate) decides
    /// the concrete impls — tests substitute mocks without forking
    /// core.
    #[must_use]
    pub fn new(
        shell: Arc<dyn ShellExecutor>,
        memory: Arc<dyn MemoryStore>,
        task_registry: Arc<TaskRegistry>,
        spawner: Arc<dyn SubagentSpawner>,
    ) -> Self {
        Self {
            manifest: PluginManifest {
                id: "core-tools".into(),
                name: "Openlet Core Tools".into(),
                version: Version::new(0, 1, 0),
                description: "Ships the core built-in tools (read, list, glob, grep, write, edit, \
                     bash, todo, ask_user, enter_plan_mode, exit_plan_mode, subagent_task, \
                     task_status) through the plugin extension surface."
                    .into(),
                author: Some("Openlet".into()),
                capabilities: vec![Capability::Tool],
                core_version_req: openlet_plugin_api::manifest::core_version_req_v0_1(),
                default_priority: 50,
                config_schema: None,
            },
            shell,
            memory,
            task_registry,
            spawner,
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
        ctx.register_tool(erase(AskUserTool::new()))?;
        // Plan-mode tools land at the bottom of the list — sibling
        // agents adding web_search/web_fetch append after, so merge
        // conflicts on this file resolve by stacking.
        ctx.register_tool(erase(EnterPlanModeTool::new(self.memory.clone())))?;
        ctx.register_tool(erase(ExitPlanModeTool::new(self.memory.clone())))?;
        ctx.register_tool(erase(SubagentTaskTool::new(self.spawner.clone())))?;
        ctx.register_tool(erase(TaskStatusTool::new(self.task_registry.clone())))?;
        Ok(())
    }
}
