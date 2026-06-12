//! Boot helpers shared by the `serve` and `doctor` paths.
//!
//! Functions here are pure or env-reading utilities that both the main
//! binary and the doctor subcommand invoke. Extracted from `main.rs` to
//! keep the entry point small.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use openlet_adapters::openrouter::OpenRouterConfig;
use openlet_core::config::Config;
use openlet_core::tools::ToolHandle;
use openlet_core::tools::registry::ToolRegistry;
use openlet_core::types::agent::{AgentId, AgentSpec};
use openlet_plugin_api::context::CoreApi;
use openlet_plugin_registry::{InstalledPlugins, install_all};

use crate::app_state::AgentResources;

/// Resolve the agent workspace root: `OPENLET_WORKSPACE` if set,
/// otherwise `<data_dir>/workspace`.
pub fn resolve_workspace_root(config: &Config) -> std::path::PathBuf {
    std::env::var("OPENLET_WORKSPACE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| config.data_dir.join("workspace"))
}

/// Resolve the model API base URL: `OPENLET_MODEL_BASE_URL` if set, else
/// the OpenAI-compat default (OpenRouter). A single trailing `/` is
/// trimmed so callers can pass `…/v1` or `…/v1/` interchangeably.
pub fn resolve_model_base_url() -> String {
    let raw = std::env::var(crate::diagnostics::MODEL_BASE_URL_ENV)
        .unwrap_or_else(|_| openlet_adapters::openrouter::DEFAULT_BASE_URL.to_string());
    raw.strip_suffix('/').unwrap_or(&raw).to_string()
}

/// Build OpenRouter request-enrichment config from env. All optional —
/// unset values send a vanilla OpenAI-shaped request.
pub fn openrouter_config_from_env() -> OpenRouterConfig {
    OpenRouterConfig {
        referer: std::env::var("OPENLET_OPENROUTER_REFERER").ok(),
        title: std::env::var("OPENLET_OPENROUTER_TITLE").ok(),
        routing: None,
        models_fallback: Vec::new(),
    }
}

/// Build the tool registry from plugin-drained handles.
pub fn build_tool_registry(tools: Vec<ToolHandle>) -> Arc<ToolRegistry> {
    let mut tool_builder = ToolRegistry::builder();
    for tool in tools {
        tool_builder = tool_builder.register_erased(tool);
    }
    tool_builder.build()
}

/// Wire the single default agent (one workspace -> one fs + shell) that
/// MVP boot registers. Returns the generated id alongside the
/// one-entry `agents` map both boot paths hand to `AppStateBuilder`.
pub fn single_default_agent(
    workspace_root: std::path::PathBuf,
    fs: Arc<dyn openlet_core::adapters::Filesystem>,
    shell: Arc<dyn openlet_core::tools::builtins::bash::ShellExecutor>,
) -> (AgentId, HashMap<AgentId, AgentResources>) {
    let default_agent_id = AgentId::new();
    let agent_spec = AgentSpec::new(default_agent_id, workspace_root, "default");
    let mut agents = HashMap::new();
    agents.insert(
        default_agent_id,
        AgentResources {
            spec: agent_spec,
            fs,
            shell,
        },
    );
    (default_agent_id, agents)
}

/// Install all compile-time plugins via `install_all` and return the
/// fully-drained registry. Called once during boot.
pub async fn install_plugins(
    core_api: Arc<dyn CoreApi>,
    shell: Arc<dyn openlet_core::tools::builtins::bash::ShellExecutor>,
    memory: Arc<dyn openlet_core::adapters::memory_store::MemoryStore>,
    task_registry: Arc<openlet_core::runtime::subagent::TaskRegistry>,
    spawner: Arc<dyn openlet_core::tools::builtins::subagent_task::SubagentSpawner>,
) -> anyhow::Result<InstalledPlugins> {
    let plugins = openlet_plugin_registry::all_plugins(shell, memory, task_registry, spawner);
    let configs = std::collections::HashMap::new();
    install_all(plugins, &configs, core_api)
        .await
        .context("draining plugin registrations")
}
