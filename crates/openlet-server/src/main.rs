//! Openlet server binary entry point.
//!
//! Bootstrap order: parse CLI → load `Config` → init tracing → build
//! `AppState` with stub adapters → serve axum on `Config::bind_addr` with
//! graceful Ctrl+C shutdown.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use futures::FutureExt;
use openlet_adapters::{
    bus::BroadcastBus,
    config_perm::ConfigPermissionMgr,
    localfs::{LocalFilesystem, LocalFsArtifactStore},
    localshell::{LocalShellExecutor, LocalShellToolExecutor},
    openai_compat::OpenAiCompatProvider,
    sqlite::SqliteMemoryStore,
};
use openlet_core::adapters::hooked_event_sink::HookedEventSink;
use openlet_core::adapters::hooked_memory_store::HookedMemoryStore;
use openlet_core::config::{Config, LogFormat};
use openlet_core::runtime::question_registry::QuestionRegistry;
use openlet_core::runtime::{ConversationRuntime, RuntimeConfig};
use openlet_core::types::agent::{AgentId, AgentSpec};
use openlet_plugin_api::context::CoreApi;
use openlet_plugin_registry::{FinalizedRegistry, install_all};
use openlet_server::{
    AgentResources, AppStateBuilder, RouterBuilder,
    cli::{Cli, Command, DoctorArgs, DoctorFormat},
};
use tokio::net::TcpListener;
use tokio::signal;
use tracing::info;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let mut config = Config::load().context("loading config")?;
    init_tracing(config.log_format);

    match cli.resolved_command() {
        Command::Serve(args) => {
            if let Some(bind) = args.bind {
                config.bind_addr = bind;
            }
            run_server(config).await
        }
        Command::Audit(args) => openlet_server::audit::run(args, &config.data_dir).await,
        Command::Doctor(args) => run_doctor(args, config).await,
    }
}

fn init_tracing(format: LogFormat) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let registry = tracing_subscriber::registry().with(filter);
    match format {
        LogFormat::Json => {
            registry
                .with(tracing_subscriber::fmt::layer().json())
                .init();
        }
        LogFormat::Pretty => {
            registry
                .with(tracing_subscriber::fmt::layer().pretty())
                .init();
        }
    }
}

async fn run_server(config: Config) -> anyhow::Result<()> {
    let db_path = config.data_dir.join("db.sqlite");
    let artifact_root = config.data_dir.join("artifacts");
    let session_log_root = config.data_dir.join("sessions");

    let pool = openlet_adapters::sqlite::open_pool(&db_path, 8)
        .await
        .with_context(|| format!("opening sqlite at {}", db_path.display()))?;
    openlet_adapters::sqlite::run_migrations(&pool)
        .await
        .context("running sqlite migrations")?;

    tokio::fs::create_dir_all(&artifact_root)
        .await
        .with_context(|| format!("create artifact dir {}", artifact_root.display()))?;
    tokio::fs::create_dir_all(&session_log_root)
        .await
        .with_context(|| format!("create session log dir {}", session_log_root.display()))?;

    let provider = OpenAiCompatProvider::new(
        openlet_adapters::openai_compat::DEFAULT_BASE_URL,
        config.openrouter_api_key.clone(),
    );

    let provider: Arc<dyn openlet_core::adapters::ModelProvider> = Arc::new(provider);
    let inner_memory: Arc<dyn openlet_core::adapters::MemoryStore> =
        Arc::new(SqliteMemoryStore::new(pool.clone()));
    let event_repo = openlet_adapters::sqlite::event_repo::SqliteEventRepo::new(pool.clone());
    let inner_events: Arc<dyn openlet_core::adapters::EventSink> =
        Arc::new(BroadcastBus::with_repo(event_repo));

    // Workspace + shell built BEFORE install_plugins so `core-tools`
    // can take ownership of the shell at registration time.
    let workspace_root = std::env::var("OPENLET_WORKSPACE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| config.data_dir.join("workspace"));
    tokio::fs::create_dir_all(&workspace_root)
        .await
        .with_context(|| format!("create workspace dir {}", workspace_root.display()))?;

    let shell_exec = Arc::new(LocalShellExecutor::new(workspace_root.clone()));
    let fs_adapter = Arc::new(LocalFilesystem::new(workspace_root.clone()));
    let shell: Arc<dyn openlet_core::tools::builtins::bash::ShellExecutor> = shell_exec.clone();

    // Drain every plugin's registrations through `install_all`. Returns
    // sorted hook chains, agents, tools, and an optional provider. The
    // resulting `Arc<HookChains>` is shared by HookedEventSink, the
    // permission manager, the conversation runtime, and the turn loop —
    // any of those sites can then dispatch real plugin hooks.
    //
    // CoreApi is constructed BEFORE install_plugins so plugin hook
    // closures can capture it; the runtime is bound late via
    // `set_runtime` after we build it below.
    let config_arc = Arc::new(config.clone());
    let core_api_impl = Arc::new(openlet_server::core_api_impl::CoreApiImpl::new(
        inner_memory.clone(),
        inner_events.clone(),
        config_arc.clone(),
    ));
    let core_api: Arc<dyn CoreApi> = core_api_impl.clone();

    // Subagent task registry + spawner — built BEFORE install_plugins
    // so `core-tools` can register `subagent_task`/`task_status` with
    // live handles. The spawner is late-bound to AppState below.
    let task_registry = Arc::new(openlet_core::runtime::subagent::TaskRegistry::from_env());
    let subagent_spawner = Arc::new(openlet_server::RuntimeSubagentSpawner::new());
    let spawner_dyn: Arc<dyn openlet_core::tools::builtins::subagent_task::SubagentSpawner> =
        subagent_spawner.clone();

    let installed = install_plugins(
        core_api,
        shell.clone(),
        inner_memory.clone(),
        task_registry.clone(),
        spawner_dyn,
    )
    .await?;
    let hook_chains = Arc::new(installed.chains);
    // First plugin to register a provider wins; otherwise fall back to
    // the OpenAI-compat provider built from `Config`.
    let provider = installed.provider.unwrap_or(provider);

    let memory: Arc<dyn openlet_core::adapters::MemoryStore> = Arc::new(HookedMemoryStore::new(
        inner_memory.clone(),
        hook_chains.clone(),
    ));
    let events: Arc<dyn openlet_core::adapters::EventSink> = Arc::new(HookedEventSink::new(
        inner_events.clone(),
        hook_chains.clone(),
    ));

    // §I: crash recovery — mark any leftover Running sessions as Errored.
    let stale = memory
        .list_sessions(openlet_core::types::session::SessionFilter {
            status: Some(openlet_core::types::session::SessionStatus::Running),
            ..Default::default()
        })
        .await
        .context("listing stale running sessions")?;
    for s in stale {
        let _ = memory
            .update_status(
                s.id,
                openlet_core::types::session::SessionStatus::Errored,
                "crashed",
            )
            .await;
        let _ = events
            .publish(
                openlet_core::types::event::AgentEvent::SessionStatus {
                    session_id: s.id,
                    status: openlet_core::types::session::SessionStatus::Errored,
                    at: chrono::Utc::now(),
                },
                openlet_core::adapters::event_sink::Persistence::Durable,
            )
            .await;
    }

    let runtime = Arc::new(ConversationRuntime::with_hook_chains(
        provider.clone(),
        memory.clone(),
        events.clone(),
        RuntimeConfig::new(config.default_model.clone()),
        hook_chains.clone(),
    ));
    // Late-bind the runtime into the CoreApi handed to plugins above.
    // Hook closures only invoke CoreApi from inside dispatch sites, so
    // the runtime is guaranteed to be set before any plugin call.
    core_api_impl.set_runtime(runtime.clone());
    // Notification dispatch reads the chain set; bind here once chains
    // are sorted but before any plugin emits.
    core_api_impl.set_hook_chains(hook_chains.clone());

    // Tool registry rebuilt from plugin-drained handles. `core-tools`
    // is the first plugin contributor (the eight built-ins); downstream
    // integrators add their own tools through the same surface.
    let mut tool_builder = openlet_core::tools::ToolRegistry::builder();
    for tool in installed.tools {
        tool_builder = tool_builder.register_erased(tool);
    }
    let tool_registry = tool_builder.build();

    let default_agent_id = AgentId::new();
    let agent_spec = AgentSpec::new(default_agent_id, workspace_root.clone(), "default");
    let mut agents = HashMap::new();
    agents.insert(
        default_agent_id,
        AgentResources {
            spec: agent_spec,
            fs: fs_adapter.clone(),
            shell: shell.clone(),
        },
    );

    // Build the agent registry from plugin-drained AgentDefinitions.
    let mut agent_registry = openlet_core::agent::AgentRegistry::new();
    for def in installed.agents {
        agent_registry
            .insert(def)
            .context("inserting plugin-drained agent definition")?;
    }

    let state = AppStateBuilder::new()
        .provider(provider)
        .memory(memory)
        .artifacts(Arc::new(LocalFsArtifactStore::new(
            artifact_root,
            pool.clone(),
        )))
        .tools(Arc::new(LocalShellToolExecutor::new()))
        .tool_registry(tool_registry)
        .events(events)
        .permission(Arc::new(
            ConfigPermissionMgr::new().with_hook_chains(hook_chains.clone()),
        ))
        .config(Arc::new(config.clone()))
        .hook_chains(hook_chains.clone())
        .runtime(runtime)
        .agents(agents)
        .default_agent_id(default_agent_id)
        .agent_registry(Arc::new(agent_registry))
        .questions(Arc::new(QuestionRegistry::new()))
        .task_registry(task_registry.clone())
        .build()
        .context("building app state")?;

    // Late-bind the live AppState into the subagent spawner so
    // `subagent_task` tool dispatches can resolve permission, agent
    // resources, and the conversation runtime. Boot order: spawner
    // built BEFORE plugins (so core-tools registers it), then bound
    // here once AppState is constructed.
    subagent_spawner.set_state(state.clone());

    // Late-bind active_turns into CoreApi so plugins can call
    // `cancel_session` from inside hook closures. Same OnceLock pattern
    // as `set_runtime` — idempotent, fires once at boot.
    core_api_impl.set_active_turns(state.active_turns.clone());

    // The local binary mounts no upstream auth layer, but the
    // `question/answer` route requires an `AuthPrincipal` extension
    // (cloud integrators attach a real one). Inject a stub so the
    // `ask_user` rendezvous works out-of-the-box locally — otherwise
    // every `ask_user` call hangs until timeout. Loopback-only +
    // no-auth is the documented MVP posture, so a constant principal
    // is the correct local behaviour.
    let app = RouterBuilder::default()
        .build(state.clone())
        .layer(axum::Extension(
            openlet_server::routes::question::AuthPrincipal,
        ));
    let listener = TcpListener::bind(&config.bind_addr)
        .await
        .with_context(|| format!("binding {}", config.bind_addr))?;
    let local_addr = listener.local_addr().ok();

    // Refuse non-loopback binds without explicit operator opt-in via
    // OPENLET_ALLOW_NON_LOOPBACK=1. The MVP threat model assumes no auth
    // in front of the API; binding to a routable address would expose
    // every endpoint (incl. permission auto-approve) to the network.
    if let Some(addr) = local_addr {
        let allow_non_loopback = std::env::var("OPENLET_ALLOW_NON_LOOPBACK")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        if !addr.ip().is_loopback() && !allow_non_loopback {
            anyhow::bail!(
                "refusing to bind non-loopback address {addr} without OPENLET_ALLOW_NON_LOOPBACK=1; \
                 the MVP server has no built-in auth and must not be exposed beyond loopback",
            );
        }
        if !addr.ip().is_loopback() {
            tracing::warn!(
                bind = %addr,
                "bound NON-LOOPBACK address — every endpoint is exposed without auth; \
                 ensure an authenticating reverse-proxy fronts this listener"
            );
        } else {
            info!(bind = %addr, "bound loopback at http://{addr}");
        }
    }

    let serve_result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serving axum");

    // Drain in-flight turn drivers before plugin shutdown. `axum::serve`
    // returns once HTTP handlers stop, but `prompt_async` spawns the
    // actual turn loop with `tokio::spawn` and returns 202 immediately —
    // those tasks are still billing, writing parts, and emitting events.
    // Tearing plugins down underneath them risks panics on disposed
    // handles. Trip each turn's cancel token, then await its `exited`
    // Notify (signalled by the driver's Drop guard) under a single 25s
    // budget — leaving ~5s for plugin shutdown inside the k8s default
    // terminationGracePeriodSeconds=30.
    let in_flight: Vec<_> = state
        .active_turns
        .iter()
        .map(|e| e.value().clone())
        .collect();
    if !in_flight.is_empty() {
        info!(count = in_flight.len(), "draining in-flight turns");
        let drain = async {
            let waits = in_flight.into_iter().map(|h| async move {
                // Enable the Notified future BEFORE tripping cancel so the
                // driver's `notify_waiters()` (fired from its Drop guard
                // once it observes the cancel) can't slip through the gap
                // between subscribe and await — `notify_waiters` wakes
                // only currently-registered waiters.
                let n = h.exited.notified();
                tokio::pin!(n);
                n.as_mut().enable();
                h.cancel.cancel();
                n.await;
            });
            futures::future::join_all(waits).await;
        };
        if tokio::time::timeout(std::time::Duration::from_secs(25), drain)
            .await
            .is_err()
        {
            tracing::warn!(
                "turn drain timed out (25s); some in-flight turns may not have finished cleanly"
            );
        }
    }

    // Drive Plugin::shutdown after axum returns. Plugins holding
    // resources (sockets, billing-flush state, audit handles) get a
    // chance to drain before the process exits. Per-plugin shutdown is
    // panic-isolated so a buggy plugin can't strand the others.
    // Phase 9 (FMA-F5 ACCEPT): run shutdowns in parallel under a single
    // 5s timeout so total wall time stays bounded by ONE timeout window
    // (not 5s × N plugins). Fits k8s default terminationGracePeriodSeconds=30.
    let shutdowns = installed.plugins.iter().map(|plugin| {
        let id = plugin.manifest().id.clone();
        async move {
            let result = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                std::panic::AssertUnwindSafe(plugin.shutdown()).catch_unwind(),
            )
            .await;
            match result {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(e))) => {
                    tracing::warn!(plugin = %id, error = %e, "plugin shutdown returned error");
                }
                Ok(Err(_)) => {
                    tracing::warn!(plugin = %id, "plugin shutdown panicked");
                }
                Err(_) => {
                    tracing::warn!(plugin = %id, "plugin shutdown timed out (5s)");
                }
            }
        }
    });
    futures::future::join_all(shutdowns).await;

    serve_result?;
    info!("openlet-server stopped");
    Ok(())
}

/// Install all compile-time plugins via `install_all` and return the
/// fully-drained registry. Called once during boot. The returned chains
/// are sorted; provider/agents/tools are ready to plumb into AppState.
///
/// `core_api` is the late-bound `CoreApiImpl` constructed before this
/// call: its `runtime` slot is filled by [`CoreApiImpl::set_runtime`]
/// after the conversation runtime is built. Plugin hook closures
/// receive `Arc<dyn CoreApi>` and only invoke it from inside dispatch
/// sites, well after boot has bound the runtime.
async fn install_plugins(
    core_api: Arc<dyn CoreApi>,
    shell: Arc<dyn openlet_core::tools::builtins::bash::ShellExecutor>,
    memory: Arc<dyn openlet_core::adapters::memory_store::MemoryStore>,
    task_registry: Arc<openlet_core::runtime::subagent::TaskRegistry>,
    spawner: Arc<dyn openlet_core::tools::builtins::subagent_task::SubagentSpawner>,
) -> anyhow::Result<FinalizedRegistry> {
    let plugins = openlet_plugin_registry::all_plugins(shell, memory, task_registry, spawner);
    let configs = std::collections::HashMap::new();
    install_all(plugins, &configs, core_api)
        .await
        .context("draining plugin registrations")
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("install Ctrl+C handler");
    };

    #[cfg(unix)]
    let term = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let term = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => info!("received Ctrl+C, shutting down"),
        () = term => info!("received SIGTERM, shutting down"),
    }
}

/// Build a minimal AppState off the live `Config`, run preflight
/// diagnostics, print the (redacted) report, and exit with a status code
/// matching the worst-case check.
async fn run_doctor(args: DoctorArgs, config: Config) -> anyhow::Result<()> {
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

/// Build a slim AppState for the `doctor` subcommand. Same wiring as
/// `run_server` but skips graceful shutdown / hook plumbing the report
/// doesn't read. Crash recovery is intentionally NOT run here — `doctor`
/// is read-only.
async fn build_doctor_state(config: &Config) -> anyhow::Result<openlet_server::AppState> {
    let db_path = config.data_dir.join("db.sqlite");
    let artifact_root = config.data_dir.join("artifacts");

    let pool = openlet_adapters::sqlite::open_pool(&db_path, 2)
        .await
        .with_context(|| format!("opening sqlite at {}", db_path.display()))?;
    openlet_adapters::sqlite::run_migrations(&pool)
        .await
        .context("running sqlite migrations")?;

    tokio::fs::create_dir_all(&artifact_root).await.ok();

    let provider: Arc<dyn openlet_core::adapters::ModelProvider> =
        Arc::new(OpenAiCompatProvider::new(
            openlet_adapters::openai_compat::DEFAULT_BASE_URL,
            config.openrouter_api_key.clone(),
        ));
    let memory: Arc<dyn openlet_core::adapters::MemoryStore> =
        Arc::new(SqliteMemoryStore::new(pool.clone()));
    let event_repo = openlet_adapters::sqlite::event_repo::SqliteEventRepo::new(pool.clone());
    let events: Arc<dyn openlet_core::adapters::EventSink> =
        Arc::new(BroadcastBus::with_repo(event_repo));

    let workspace_root = std::env::var("OPENLET_WORKSPACE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| config.data_dir.join("workspace"));
    tokio::fs::create_dir_all(&workspace_root).await.ok();

    let shell_exec = Arc::new(LocalShellExecutor::new(workspace_root.clone()));
    let fs_adapter = Arc::new(LocalFilesystem::new(workspace_root.clone()));
    let shell: Arc<dyn openlet_core::tools::builtins::bash::ShellExecutor> = shell_exec.clone();

    let core_api: Arc<dyn openlet_plugin_api::context::CoreApi> =
        Arc::new(openlet_server::core_api_impl::CoreApiImpl::new(
            memory.clone(),
            events.clone(),
            Arc::new(config.clone()),
        ));
    let task_registry_dr = Arc::new(openlet_core::runtime::subagent::TaskRegistry::from_env());
    let subagent_spawner_dr = Arc::new(openlet_server::RuntimeSubagentSpawner::new());
    let spawner_dyn_dr: Arc<dyn openlet_core::tools::builtins::subagent_task::SubagentSpawner> =
        subagent_spawner_dr.clone();
    let installed = install_plugins(
        core_api,
        shell.clone(),
        memory.clone(),
        task_registry_dr.clone(),
        spawner_dyn_dr,
    )
    .await?;
    let provider = installed.provider.unwrap_or(provider);
    let hook_chains = Arc::new(installed.chains);

    let mut tool_builder = openlet_core::tools::ToolRegistry::builder();
    for tool in installed.tools {
        tool_builder = tool_builder.register_erased(tool);
    }
    let tool_registry = tool_builder.build();

    let default_agent_id = AgentId::new();
    let agent_spec = AgentSpec::new(default_agent_id, workspace_root.clone(), "default");
    let mut agents = HashMap::new();
    agents.insert(
        default_agent_id,
        AgentResources {
            spec: agent_spec,
            fs: fs_adapter,
            shell,
        },
    );

    let mut plugin_registry = openlet_plugin_registry::PluginRegistry::new();
    for plugin in installed.plugins {
        plugin_registry.push(plugin);
    }

    AppStateBuilder::new()
        .provider(provider)
        .memory(memory)
        .artifacts(Arc::new(LocalFsArtifactStore::new(
            artifact_root,
            pool.clone(),
        )))
        .tools(Arc::new(LocalShellToolExecutor::new()))
        .tool_registry(tool_registry)
        .events(events)
        .permission(Arc::new(ConfigPermissionMgr::new()))
        .config(Arc::new(config.clone()))
        .plugin_registry(Arc::new(plugin_registry))
        .hook_chains(hook_chains)
        .agents(agents)
        .default_agent_id(default_agent_id)
        .agent_registry(Arc::new(openlet_core::agent::AgentRegistry::new()))
        .build()
        .context("building doctor app state")
}
