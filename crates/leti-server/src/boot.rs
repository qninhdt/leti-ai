//! Boot helpers shared by the `serve` and `doctor` paths.
//!
//! Functions here are pure or env-reading utilities that both the main
//! binary and the doctor subcommand invoke. Extracted from `main.rs` to
//! keep the entry point small.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use leti_adapters::openrouter::OpenRouterConfig;
use leti_core::config::Config;
use leti_core::tools::ToolHandle;
use leti_core::tools::registry::ToolRegistry;
use leti_core::types::agent::{AgentId, AgentSpec};
use leti_plugin_api::context::CoreApi;
use leti_plugin_registry::{InstalledPlugins, install_all};
use secrecy::SecretString;

use crate::app_state::AgentResources;

/// Resolve the agent workspace root: `LETI_WORKSPACE` if set,
/// otherwise `<data_dir>/workspace`.
pub fn resolve_workspace_root(config: &Config) -> std::path::PathBuf {
    std::env::var("LETI_WORKSPACE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| config.data_dir.join("workspace"))
}

/// Resolve the model API base URL: `OPENAI_API_BASE_URL` if set, else
/// the OpenAI-compat default (OpenRouter). A single trailing `/` is
/// trimmed so callers can pass `…/v1` or `…/v1/` interchangeably.
pub fn resolve_model_base_url() -> String {
    let raw = std::env::var(crate::diagnostics::MODEL_BASE_URL_ENV)
        .unwrap_or_else(|_| leti_adapters::openrouter::DEFAULT_BASE_URL.to_string());
    raw.strip_suffix('/').unwrap_or(&raw).to_string()
}

/// Build OpenRouter request-enrichment config from env. All optional —
/// unset values send a vanilla OpenAI-shaped request.
pub fn openrouter_config_from_env() -> OpenRouterConfig {
    OpenRouterConfig {
        referer: std::env::var("LETI_OPENROUTER_REFERER").ok(),
        title: std::env::var("LETI_OPENROUTER_TITLE").ok(),
        routing: None,
        models_fallback: Vec::new(),
    }
}

/// Read the provider credential at the server composition root. The engine
/// configuration remains free of provider secrets.
#[must_use]
pub fn openai_api_key_from_env() -> Option<SecretString> {
    std::env::var("OPENAI_API_KEY").ok().map(SecretString::from)
}

/// Build the tool registry from plugin-drained handles.
///
/// `LETI_DISABLED_TOOLS` (comma-separated tool names, e.g. `bash` or
/// `bash,edit`) drops matching tools before registration so the model never
/// sees them in its tool catalog and can't dispatch them. Whitespace around
/// each name is trimmed; empty entries are ignored. Unknown names are a no-op.
pub fn build_tool_registry(tools: Vec<ToolHandle>) -> Arc<ToolRegistry> {
    let disabled = disabled_tool_names();
    let mut tool_builder = ToolRegistry::builder();
    for tool in tools {
        if disabled.iter().any(|d| d == tool.name()) {
            tracing::info!(tool = tool.name(), "tool disabled via LETI_DISABLED_TOOLS");
            continue;
        }
        tool_builder = tool_builder.register_erased(tool);
    }
    tool_builder.build()
}

/// Parse `LETI_DISABLED_TOOLS` into a trimmed, non-empty name list.
fn disabled_tool_names() -> Vec<String> {
    std::env::var("LETI_DISABLED_TOOLS")
        .map(|raw| parse_disabled_tools(&raw))
        .unwrap_or_default()
}

/// Pure comma-split + trim + drop-empties. Split out from the env reader so
/// the parsing is unit-testable without mutating process env.
fn parse_disabled_tools(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Wire the single default agent (one workspace -> one fs + shell) that
/// MVP boot registers. Returns the generated id alongside the
/// one-entry `agents` map both boot paths hand to `AppStateBuilder`.
pub fn single_default_agent(
    workspace_root: std::path::PathBuf,
    fs: Arc<dyn leti_core::adapters::Filesystem>,
    shell: Arc<dyn leti_core::tools::builtins::bash::ShellExecutor>,
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

/// Crash recovery — mark any leftover `Running` sessions as `Errored` and
/// publish the durable status transition. A `Running` row at boot means the
/// process died mid-turn; without this sweep the session would look live
/// forever. Extracted verbatim from `main.rs` boot.
pub async fn recover_stale_running_sessions(
    memory: &Arc<dyn leti_core::adapters::MemoryStore>,
    events: &Arc<dyn leti_core::adapters::EventSink>,
) -> anyhow::Result<()> {
    let stale = memory
        .list_sessions(leti_core::types::session::SessionFilter {
            status: Some(leti_core::types::session::SessionStatus::Running),
            ..Default::default()
        })
        .await
        .context("listing stale running sessions")?;
    for s in stale {
        let _ = memory
            .update_status(
                s.id,
                leti_core::types::session::SessionStatus::Errored,
                "crashed",
            )
            .await;
        let _ = events
            .publish(
                leti_core::types::event::AgentEvent::SessionStatus {
                    session_id: s.id,
                    status: leti_core::types::session::SessionStatus::Errored,
                    at: chrono::Utc::now(),
                },
                leti_core::adapters::event_sink::Persistence::Durable,
            )
            .await;
    }
    let interrupted = memory
        .interrupt_live_subagent_executions("process_restart")
        .await
        .context("interrupting stale subagent executions")?;
    for execution in interrupted {
        let _ = events
            .publish(
                leti_core::types::event::AgentEvent::SubagentSettled {
                    task_id: execution.task_id.0,
                    child_session_id: execution.child_session_id,
                    parent_session_id: execution.parent_session_id,
                    status: "interrupted".to_string(),
                    cost_usd: execution.cost_usd,
                },
                leti_core::adapters::event_sink::Persistence::Durable,
            )
            .await;
    }
    Ok(())
}

/// Fail-closed guard on the resolved listener address. Refuses a
/// non-loopback bind unless BOTH an explicit operator opt-in
/// (`LETI_ALLOW_NON_LOOPBACK=1`) AND a non-dev authenticator are in place —
/// a dev authenticator admits every request as one principal, so exposing it
/// beyond loopback would hand the whole API to the network. Extracted verbatim
/// from `main.rs` boot.
pub fn assert_bind_safe(
    addr: std::net::SocketAddr,
    authenticator_is_dev: bool,
) -> anyhow::Result<()> {
    let allow_non_loopback = std::env::var("LETI_ALLOW_NON_LOOPBACK")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if !addr.ip().is_loopback() {
        if !allow_non_loopback {
            anyhow::bail!(
                "refusing to bind non-loopback address {addr} without \
                 LETI_ALLOW_NON_LOOPBACK=1",
            );
        }
        if authenticator_is_dev {
            anyhow::bail!(
                "refusing to bind non-loopback address {addr} with the dev authenticator; \
                 it admits every request as one principal. Run with \
                 a real Authenticator before exposing beyond loopback",
            );
        }
        tracing::warn!(
            bind = %addr,
            "bound NON-LOOPBACK address with a non-dev authenticator; \
             ensure the deployment fronts this listener appropriately"
        );
    } else {
        tracing::info!(bind = %addr, "bound loopback at http://{addr}");
    }
    Ok(())
}

/// Install all compile-time plugins via `install_all` and return the
/// fully-drained registry. Called once during boot.
pub async fn install_plugins(
    core_api: Arc<dyn CoreApi>,
    shell: Arc<dyn leti_core::tools::builtins::bash::ShellExecutor>,
    python: Option<Arc<dyn leti_core::tools::builtins::python::PythonExecutor>>,
    web_fetcher: Option<Arc<dyn leti_core::tools::builtins::web_fetch::WebFetcher>>,
    memory: Arc<dyn leti_core::adapters::memory_store::MemoryStore>,
    task_registry: Arc<leti_core::runtime::subagent::TaskRegistry>,
    spawner: Arc<dyn leti_core::tools::builtins::subagent_task::SubagentSpawner>,
) -> anyhow::Result<InstalledPlugins> {
    let plugins = leti_plugin_registry::all_plugins(
        shell,
        python,
        web_fetcher,
        memory,
        task_registry,
        spawner,
    );
    let configs = std::collections::HashMap::new();
    install_all(plugins, &configs, core_api)
        .await
        .context("draining plugin registrations")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_disabled_tools_trims_and_drops_empties() {
        assert_eq!(parse_disabled_tools("bash"), vec!["bash"]);
        assert_eq!(parse_disabled_tools(" bash , edit "), vec!["bash", "edit"]);
        assert_eq!(parse_disabled_tools("bash,,"), vec!["bash"]);
        assert!(parse_disabled_tools("").is_empty());
        assert!(parse_disabled_tools("  ,  ").is_empty());
    }
}
