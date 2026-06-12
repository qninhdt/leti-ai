//! `doctor` subcommand — preflight diagnostics.
//!
//! Builds a slim `AppState` off the live `Config`, runs the diagnostic
//! checks, prints a (redacted) report, and exits with a status code
//! matching the worst-case check. Read-only: crash recovery and graceful
//! shutdown wiring that `run_server` performs are intentionally skipped.

use std::sync::Arc;

use anyhow::Context;
use openlet_adapters::config_perm::ConfigPermissionMgr;
use openlet_core::config::Config;
use openlet_server::AppStateBuilder;
use openlet_server::cli::{DoctorArgs, DoctorFormat};

use openlet_server::boot::{
    build_tool_registry, install_plugins, openrouter_config_from_env, resolve_model_base_url,
    resolve_workspace_root, single_default_agent,
};

/// Build a minimal AppState off the live `Config`, run preflight
/// diagnostics, print the (redacted) report, and exit with a status code
/// matching the worst-case check.
pub(crate) async fn run_doctor(args: DoctorArgs, config: Config) -> anyhow::Result<()> {
    use openlet_server::diagnostics::run_diagnostics;

    let state = build_doctor_state(&config).await?;
    let report = run_diagnostics(&state).await;

    match args.format {
        DoctorFormat::Json => {
            let value = report.to_redacted_json();
            println!(
                "{}",
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".to_string())
            );
        }
        DoctorFormat::Text => print_doctor_text(&report),
    }

    let code = report.exit_code();
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

fn print_doctor_text(report: &openlet_server::diagnostics::DoctorReport) {
    use openlet_server::diagnostics::Status;
    for c in &report.checks {
        let glyph = match c.status {
            Status::Healthy => "[OK]",
            Status::Degraded => "[WARN]",
            Status::Failed => "[FAIL]",
        };
        match &c.detail {
            Some(d) => println!("{glyph} {} ({} ms) — {}", c.name, c.elapsed_ms, d),
            None => println!("{glyph} {} ({} ms)", c.name, c.elapsed_ms),
        }
    }
    let overall = match report.overall {
        Status::Healthy => "Healthy",
        Status::Degraded => "Degraded",
        Status::Failed => "Failed",
    };
    println!("\nOverall: {overall}");
}

/// Build a slim AppState for the `doctor` subcommand. Same adapter stack
/// as `run_server` but skips graceful shutdown / hook plumbing the report
/// doesn't read. Crash recovery is intentionally NOT run here — `doctor`
/// is read-only.
async fn build_doctor_state(config: &Config) -> anyhow::Result<openlet_server::AppState> {
    use openlet_adapters::openrouter::OpenRouterProvider;

    let stack = openlet_server::adapter_stack::AdapterStack::build(
        openlet_server::adapter_stack::AdapterStackConfig {
            config,
            provider: Arc::new(OpenRouterProvider::new(
                resolve_model_base_url(),
                config.openrouter_api_key.clone(),
                openrouter_config_from_env(),
            )),
            workspace_root: resolve_workspace_root(config),
            pool_size: 2,
            strict_dirs: false,
        },
    )
    .await?;

    let core_api: Arc<dyn openlet_plugin_api::context::CoreApi> =
        Arc::new(openlet_server::core_api_impl::CoreApiImpl::new(
            stack.memory.clone(),
            stack.events.clone(),
            Arc::new(config.clone()),
        ));
    let task_registry = Arc::new(openlet_core::runtime::subagent::TaskRegistry::from_env());
    let subagent_spawner = Arc::new(openlet_server::RuntimeSubagentSpawner::new());
    let spawner_dyn: Arc<dyn openlet_core::tools::builtins::subagent_task::SubagentSpawner> =
        subagent_spawner.clone();
    let installed = install_plugins(
        core_api,
        stack.shell.clone(),
        stack.memory.clone(),
        task_registry.clone(),
        spawner_dyn,
    )
    .await?;
    let provider = installed.provider.unwrap_or(stack.provider);
    let hook_chains = Arc::new(installed.chains);

    let tool_registry = build_tool_registry(installed.tools);

    let (default_agent_id, agents) =
        single_default_agent(stack.workspace_root.clone(), stack.fs, stack.shell);
    let workspace_root = stack.workspace_root;

    let mut plugin_registry = openlet_plugin_registry::PluginHandles::new();
    for plugin in installed.plugins {
        plugin_registry.push(plugin);
    }

    AppStateBuilder::new()
        .provider(provider)
        .memory(stack.memory)
        .artifacts(stack.artifacts)
        .tool_registry(tool_registry)
        .events(stack.events)
        .permission(Arc::new(ConfigPermissionMgr::new()))
        .config(Arc::new(config.clone()))
        .plugin_registry(Arc::new(plugin_registry))
        .hook_chains(hook_chains)
        .agents(agents)
        .default_agent_id(default_agent_id)
        .workspace_root(workspace_root)
        .agent_registry(Arc::new(openlet_core::agent::AgentRegistry::new()))
        .build()
        .context("building doctor app state")
}
