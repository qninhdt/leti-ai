//! Compile-time plugin registry.
//!
//! Built-in agents register through the plugin
//! surface so external Cloud agents can extend or replace them without
//! forking core. The `core-agents` plugin ships general + indexer.

use std::collections::HashSet;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures::FutureExt;
use openlet_core::adapters::memory_store::MemoryStore;
use openlet_core::adapters::model_provider::ModelProvider;
use openlet_core::agent::AgentDefinition;
use openlet_core::runtime::subagent::TaskRegistry;
use openlet_core::tools::ToolHandle;
use openlet_core::tools::builtins::bash::ShellExecutor;
use openlet_core::tools::builtins::subagent_task::SubagentSpawner;
use openlet_plugin_api::context::{CoreApi, PluginContext};
use openlet_plugin_api::dispatch::HookChains;
use openlet_plugin_api::manifest::PluginManifest;
use openlet_plugin_api::plugin::{Plugin, PluginError};
use semver::Version;

/// Returns the compile-time list of plugins shipped in this build.
///
/// `shell` flows into `core-tools::CoreToolsPlugin` so the `bash` tool
/// can dispatch into the host's `LocalShellExecutor`. `memory` flows in
/// for the plan-mode tools — they call `MemoryStore::switch_agent` to
/// flip the active agent profile. `task_registry` + `spawner` thread the
/// in-process subagent bookkeeping into `subagent_task` / `task_status`
/// registered by the core-tools plugin. The same shell + memory stay
/// available to other plugins through `CoreApi` if they need them.
#[must_use]
pub fn all_plugins(
    shell: Arc<dyn ShellExecutor>,
    memory: Arc<dyn MemoryStore>,
    task_registry: Arc<TaskRegistry>,
    spawner: Arc<dyn SubagentSpawner>,
) -> Vec<Arc<dyn Plugin>> {
    vec![
        Arc::new(openlet_plugin_core_agents::CoreAgentsPlugin::new()),
        Arc::new(openlet_plugin_core_tools::CoreToolsPlugin::new(
            shell,
            memory,
            task_registry,
            spawner,
        )),
    ]
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

/// Output of [`install_all`] — every plugin's drained registrations
/// merged + chains canonically sorted, ready to plumb into `AppState`.
pub struct FinalizedRegistry {
    pub plugins: Vec<Arc<dyn Plugin>>,
    pub manifests: Vec<PluginManifest>,
    pub agents: Vec<AgentDefinition>,
    pub tools: Vec<ToolHandle>,
    pub provider: Option<Arc<dyn ModelProvider>>,
    pub chains: HookChains,
}

/// Drive every plugin's `install` hook, drain its [`PluginContext`],
/// merge into a single [`FinalizedRegistry`], and sort all hook chains.
///
/// `configs` maps `manifest.id -> per-plugin config block`. Plugins
/// without an entry receive `serde_json::Value::Null`. The first
/// plugin to register a provider wins; subsequent registrations are
/// ignored with a logged warning (no error — provider conflicts are
/// non-fatal at boot).
pub async fn install_all(
    plugins: Vec<Arc<dyn Plugin>>,
    configs: &std::collections::HashMap<String, serde_json::Value>,
    core_api: Arc<dyn CoreApi>,
) -> Result<FinalizedRegistry, PluginError> {
    let mut manifests = Vec::with_capacity(plugins.len());
    let mut agents = Vec::new();
    let mut tools = Vec::new();
    let mut provider: Option<Arc<dyn ModelProvider>> = None;
    let mut chains = HookChains::new();
    let mut seen_ids: HashSet<String> = HashSet::with_capacity(plugins.len());

    // Resolve `core` version once per boot from the workspace package
    // version. Plugins declare a `core_version_req` semver range — boot
    // refuses any plugin whose req doesn't match.
    let core_version: Version = env!("CARGO_PKG_VERSION")
        .parse()
        .expect("CARGO_PKG_VERSION is a valid semver");

    for plugin in &plugins {
        let manifest = plugin.manifest();

        if !seen_ids.insert(manifest.id.clone()) {
            return Err(PluginError::Runtime(format!(
                "duplicate plugin id '{}': two plugins claim the same identifier",
                manifest.id
            )));
        }

        if !manifest.core_version_req.matches(&core_version) {
            return Err(PluginError::IncompatibleCoreVersion {
                id: manifest.id.clone(),
                req: manifest.core_version_req.to_string(),
                have: core_version.to_string(),
            });
        }

        let raw_cfg = configs
            .get(&manifest.id)
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let mut ctx = PluginContext::new(manifest.clone(), raw_cfg, Arc::clone(&core_api));

        // Catch panics inside `install` so a buggy plugin can't crash
        // server boot. The error surfaces as PluginError::Install with
        // a descriptive message; operators can disable the plugin via
        // config and retry.
        let install_result = AssertUnwindSafe(plugin.install(&mut ctx))
            .catch_unwind()
            .await;
        match install_result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(payload) => {
                // Surface the panic message so operators see WHY install
                // panicked, not just THAT it did. Boxed `Any` is either
                // `&'static str` or `String` for `panic!`-emitted payloads.
                let msg = if let Some(s) = payload.downcast_ref::<&'static str>() {
                    (*s).to_string()
                } else if let Some(s) = payload.downcast_ref::<String>() {
                    s.clone()
                } else {
                    "<non-string panic payload>".to_string()
                };
                return Err(PluginError::Runtime(format!(
                    "plugin '{}' install panicked: {msg}",
                    manifest.id
                )));
            }
        }

        let regs = ctx.into_registrations();

        if let Some(new_provider) = regs.provider {
            if provider.is_some() {
                tracing::warn!(
                    plugin = %manifest.id,
                    "provider already registered by an earlier plugin; ignoring later registration",
                );
            } else {
                provider = Some(new_provider);
            }
        }

        agents.extend(regs.agents);
        // Reject duplicate tool ids before extending; first-registration
        // wins. Without this, a plugin shadowing a built-in (or two
        // plugins racing for the same name) silently shipped both
        // entries and `ToolRegistry` lookup would pick by insertion
        // order — a non-obvious mis-routing.
        for tool in regs.tools {
            if tools
                .iter()
                .any(|existing: &ToolHandle| existing.name() == tool.name())
            {
                return Err(PluginError::Runtime(format!(
                    "tool id collision: '{}' already registered by an earlier plugin",
                    tool.name()
                )));
            }
            tools.push(tool);
        }

        chains.before_turn.extend(regs.chains.before_turn);
        chains.after_turn.extend(regs.chains.after_turn);
        chains.on_chat_params.extend(regs.chains.on_chat_params);
        chains.on_chat_messages.extend(regs.chains.on_chat_messages);
        chains.on_chat_headers.extend(regs.chains.on_chat_headers);
        chains.before_tool_call.extend(regs.chains.before_tool_call);
        chains.after_tool_call.extend(regs.chains.after_tool_call);
        chains
            .on_permission_ask
            .extend(regs.chains.on_permission_ask);
        chains.on_message.extend(regs.chains.on_message);
        chains.on_cost_tick.extend(regs.chains.on_cost_tick);
        chains.on_step_finish.extend(regs.chains.on_step_finish);
        chains.on_compaction.extend(regs.chains.on_compaction);
        chains
            .on_session_status
            .extend(regs.chains.on_session_status);
        chains.on_event.extend(regs.chains.on_event);
        chains.notification.extend(regs.chains.notification);

        manifests.push(manifest.clone());
    }

    chains.sort_all();

    Ok(FinalizedRegistry {
        plugins,
        manifests,
        agents,
        tools,
        provider,
        chains,
    })
}
