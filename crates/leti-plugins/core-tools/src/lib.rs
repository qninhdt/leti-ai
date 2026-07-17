//! `core-tools` plugin — registers the built-in tools (read, list,
//! glob, grep, write, edit, bash, todo, ask_user, enter_plan_mode,
//! exit_plan_mode, subagent_task, subagent lifecycle controls, task_status) through the public
//! `register_tool` extension point. This is the dogfood
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
use leti_core::adapters::memory_store::MemoryStore;
use leti_core::runtime::subagent::TaskRegistry;
use leti_core::tools::Tool;
use leti_core::tools::builtins::bash::ShellExecutor;
use leti_core::tools::builtins::python::PythonExecutor;
use leti_core::tools::builtins::subagent_task::SubagentSpawner;
use leti_core::tools::builtins::web_fetch::WebFetcher;
use leti_core::tools::builtins::{
    AskUserTool, BashTool, EditTool, EnterPlanModeTool, ExitPlanModeTool, GlobTool, GrepTool,
    ListTool, PythonTool, ReadTool, SendMessageTool, SubagentCancelTool, SubagentContinueTool,
    SubagentInterruptTool, SubagentListTool, SubagentTaskTool, TaskStatusTool, TodoTool,
    WebFetchTool, WriteTool,
};
use leti_plugin_api::manifest::Capability;
use leti_plugin_api::{Plugin, PluginContext, PluginError, PluginManifest};
use semver::Version;

/// Erase a typed `Tool` impl into the object-safe handle the plugin
/// API stores. Mirrors `ToolRegistryBuilder::register` in spirit but
/// produces a `ToolHandle` we can hand to `ctx.register_tool`.
fn erase<T>(tool: T) -> leti_core::tools::ToolHandle
where
    T: Tool + 'static,
{
    Arc::new(tool)
}

pub struct CoreToolsPlugin {
    manifest: PluginManifest,
    shell: Arc<dyn ShellExecutor>,
    /// Optional Python executor. `None` (the default) means the `python`
    /// tool is not registered at all — integrators that don't wire a
    /// `PythonExecutor` keep exactly today's tool set, so adding this
    /// parameter is not a breaking change for them.
    python: Option<Arc<dyn PythonExecutor>>,
    /// Optional outbound web fetcher. `None` (the default) means the
    /// `web_fetch` tool is not registered — network-free integrators keep
    /// exactly today's tool set, mirroring `python`.
    web_fetcher: Option<Arc<dyn WebFetcher>>,
    memory: Arc<dyn MemoryStore>,
    task_registry: Arc<TaskRegistry>,
    spawner: Arc<dyn SubagentSpawner>,
}

impl CoreToolsPlugin {
    /// `shell` is the bash executor the `bash` tool forwards into.
    /// `memory` is the session store that `enter_plan_mode` /
    /// `exit_plan_mode` mutate to flip the active agent profile.
    /// `task_registry` + `spawner` are required by `subagent_task` and
    /// lifecycle controls. The host (server crate) builds them once at boot
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
            manifest: PluginManifest::builder("core-tools", "Leti Core Tools")
                .version(Version::new(0, 1, 0))
                .description(
                    "Ships the core built-in tools (read, list, glob, grep, write, edit, \
                     bash, todo, ask_user, enter_plan_mode, exit_plan_mode, subagent_task, \
                     subagent lifecycle controls, task_status, and the optional web_fetch) through the plugin extension \
                     surface.",
                )
                .author("Leti")
                .capabilities(vec![Capability::Tool])
                .default_priority(50)
                .build(),
            shell,
            python: None,
            web_fetcher: None,
            memory,
            task_registry,
            spawner,
        }
    }

    /// Enable the `python` tool by wiring a [`PythonExecutor`] (production:
    /// `leti_adapters::pyexec::MontyExecutor`). Builder-style so the
    /// four-arg `new` stays source-compatible for every existing integrator
    /// and test — only callers that opt in gain the tool.
    #[must_use]
    pub fn with_python(mut self, python: Arc<dyn PythonExecutor>) -> Self {
        self.python = Some(python);
        self
    }

    /// Enable the `web_fetch` tool by wiring a [`WebFetcher`] (production:
    /// `leti_adapters::webfetch::ReqwestWebFetcher`). Builder-style,
    /// mirroring [`Self::with_python`] — integrators that don't opt in keep
    /// today's network-free tool set. Egress is additionally gated by the
    /// `web_fetch:**` Ask permission seed (see `permission_seed.rs`).
    #[must_use]
    pub fn with_web_fetcher(mut self, web_fetcher: Arc<dyn WebFetcher>) -> Self {
        self.web_fetcher = Some(web_fetcher);
        self
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
        // `python` only registers when the host wired an executor — no
        // executor means the tool is absent, preserving the old tool set.
        if let Some(python) = &self.python {
            ctx.register_tool(erase(PythonTool::with_executor(python.clone())))?;
        }
        ctx.register_tool(erase(TodoTool))?;
        ctx.register_tool(erase(AskUserTool::new()))?;
        // Plan-mode tools land at the bottom of the list — sibling
        // agents adding web_search/web_fetch append after, so merge
        // conflicts on this file resolve by stacking.
        ctx.register_tool(erase(EnterPlanModeTool::new(self.memory.clone())))?;
        ctx.register_tool(erase(ExitPlanModeTool::new(self.memory.clone())))?;
        ctx.register_tool(erase(SubagentTaskTool::new(self.spawner.clone())))?;
        ctx.register_tool(erase(SubagentListTool::new(self.memory.clone())))?;
        ctx.register_tool(erase(SubagentCancelTool::new(
            self.memory.clone(),
            self.task_registry.clone(),
        )))?;
        ctx.register_tool(erase(SubagentInterruptTool::new(
            self.memory.clone(),
            self.task_registry.clone(),
        )))?;
        ctx.register_tool(erase(SubagentContinueTool::new(self.spawner.clone())))?;
        ctx.register_tool(erase(TaskStatusTool::with_memory(
            self.task_registry.clone(),
            self.memory.clone(),
        )))?;
        ctx.register_tool(erase(SendMessageTool::new(self.task_registry.clone())))?;
        // `web_fetch` registers only when the host wired a fetcher — absent
        // otherwise, so network-free integrators keep today's tool set. This
        // is the runtime's only outbound-network capability; egress is gated
        // by the `web_fetch:**` Ask permission seed.
        if let Some(web_fetcher) = &self.web_fetcher {
            ctx.register_tool(erase(WebFetchTool::with_fetcher(web_fetcher.clone())))?;
        }
        Ok(())
    }
}
