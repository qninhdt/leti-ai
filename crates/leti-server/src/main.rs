//! Leti server binary entry point.
//!
//! Bootstrap order: parse CLI → load `Config` → init tracing → build
//! `AppState` with stub adapters → serve axum on `Config::bind_addr` with
//! graceful Ctrl+C shutdown.

use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use leti_adapters::config_perm::ConfigPermissionMgr;
use leti_adapters::openrouter::OpenRouterProvider;
use leti_core::adapters::hooked_event_sink::HookedEventSink;
use leti_core::adapters::hooked_memory_store::HookedMemoryStore;
use leti_core::config::{Config, LogFormat};
use leti_core::runtime::question_registry::QuestionRegistry;
use leti_core::runtime::{ConversationRuntime, RuntimeConfig};
use leti_plugin_api::context::CoreApi;
use leti_server::boot::{
    build_tool_registry, install_plugins, openai_api_key_from_env, openrouter_config_from_env,
    recover_stale_running_sessions, resolve_model_base_url, resolve_workspace_root,
    single_default_agent,
};
use leti_server::permission_seed::default_permission_rules;
use leti_server::{
    AppStateBuilder, RouterBuilder,
    cli::{Cli, Command},
};
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

mod doctor_cmd;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    // Load `.env` from CWD (walking up to the repo root) before reading
    // config. Existing process-env vars win — dotenvy does not override
    // already-set keys, so an explicit `OPENAI_API_KEY=… cargo run`
    // still takes precedence over the file.
    let dotenv_path = dotenvy::dotenv().ok();
    let mut config = Config::load().context("loading config")?;
    init_tracing(config.log_format);
    if let Some(path) = dotenv_path {
        // Never log the values — only that a file was found and which one.
        info!(dotenv = %path.display(), "loaded .env");
    }

    match cli.resolved_command() {
        Command::Serve(args) => {
            if let Some(bind) = args.bind {
                config.bind_addr = bind;
            }
            let interaction_mode = if args.detached {
                leti_core::types::session::InteractionMode::Detached {
                    on_ask: match args.on_ask {
                        leti_server::cli::DetachedOnAsk::Allow => {
                            leti_core::types::session::DetachedAsk::Allow
                        }
                        leti_server::cli::DetachedOnAsk::Deny => {
                            leti_core::types::session::DetachedAsk::Deny
                        }
                    },
                }
            } else {
                leti_core::types::session::InteractionMode::Interactive
            };
            run_server(config, interaction_mode).await
        }
        Command::Audit(args) => leti_server::audit::run(args, &config.data_dir).await,
        Command::Doctor(args) => doctor_cmd::run_doctor(args, config).await,
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

async fn run_server(
    config: Config,
    default_interaction_mode: leti_core::types::session::InteractionMode,
) -> anyhow::Result<()> {
    // Install the metrics recorder BEFORE any emission, but ONLY when a
    // scrape bind is configured. Unset → no recorder → `metrics` macros
    // are no-ops and the binary runs as plain software (no Prometheus,
    // no infra). The handle + bind are carried to the spawn below.
    let metrics_setup = match leti_server::metrics::metrics_bind_from_env() {
        Some(bind) => Some((bind, leti_server::metrics::install_recorder()?)),
        None => None,
    };

    let stack = leti_server::adapter_stack::AdapterStack::build(
        leti_server::adapter_stack::AdapterStackConfig {
            config: &config,
            provider: Arc::new(OpenRouterProvider::new(
                resolve_model_base_url(),
                openai_api_key_from_env(),
                openrouter_config_from_env(),
            )),
            workspace_root: resolve_workspace_root(&config),
            pool_size: 8,
            strict_dirs: true,
        },
    )
    .await?;

    let session_log_root = config.data_dir.join("sessions");
    tokio::fs::create_dir_all(&session_log_root)
        .await
        .with_context(|| format!("create session log dir {}", session_log_root.display()))?;

    info!(model_base_url = %resolve_model_base_url(), "model backend endpoint");

    let provider = stack.provider;
    let inner_memory = stack.memory;
    let inner_events = stack.events;
    let workspace_root = stack.workspace_root;
    let fs_adapter = stack.fs;
    let shell = stack.shell;
    let python = stack.python;
    let web_fetcher = stack.web_fetcher;
    let artifacts = stack.artifacts;

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
    let core_api_impl = Arc::new(leti_server::core_api_impl::CoreApiImpl::new(
        inner_memory.clone(),
        inner_events.clone(),
        config_arc.clone(),
    ));
    let core_api: Arc<dyn CoreApi> = core_api_impl.clone();

    // Subagent task registry + spawner — built BEFORE install_plugins
    // so `core-tools` can register `subagent_task`/`task_status` with
    // live handles. The spawner is late-bound to AppState below.
    let task_registry = Arc::new(leti_core::runtime::subagent::TaskRegistry::from_env());
    let subagent_spawner = Arc::new(leti_server::RuntimeSubagentSpawner::new());
    let spawner_dyn: Arc<dyn leti_core::tools::builtins::subagent_task::SubagentSpawner> =
        subagent_spawner.clone();

    let installed = install_plugins(
        core_api,
        shell.clone(),
        Some(python.clone()),
        Some(web_fetcher.clone()),
        inner_memory.clone(),
        task_registry.clone(),
        spawner_dyn,
    )
    .await?;
    let hook_chains = Arc::new(installed.chains);
    // First plugin to register a provider wins; otherwise fall back to
    // the OpenAI-compat provider built from `Config`.
    let provider = installed.provider.unwrap_or(provider);

    let memory: Arc<dyn leti_core::adapters::MemoryStore> = Arc::new(HookedMemoryStore::new(
        inner_memory.clone(),
        hook_chains.clone(),
    ));
    let events: Arc<dyn leti_core::adapters::EventSink> = Arc::new(HookedEventSink::new(
        inner_events.clone(),
        hook_chains.clone(),
    ));

    // Crash recovery — mark any leftover Running sessions as Errored.
    recover_stale_running_sessions(&memory, &events).await?;

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
    let tool_registry = build_tool_registry(installed.tools);

    let (default_agent_id, agents) =
        single_default_agent(workspace_root.clone(), fs_adapter.clone(), shell.clone());

    // Build the agent registry from plugin-drained AgentDefinitions.
    let mut agent_registry = leti_core::agent::AgentRegistry::new();
    for def in installed.agents {
        agent_registry
            .insert(def)
            .context("inserting plugin-drained agent definition")?;
    }

    // Materialize the plugin registry that backs `/v1/plugin*` and the
    // graceful shutdown loop. The handles are `Arc`, so cloning into the
    // registry is cheap.
    let mut plugin_registry = leti_plugin_registry::PluginHandles::new();
    for plugin in &installed.plugins {
        plugin_registry.push(plugin.clone());
    }

    let state = AppStateBuilder::new()
        .provider(provider)
        .memory(memory)
        .artifacts(artifacts)
        .tool_registry(tool_registry)
        .events(events)
        .permission(Arc::new(
            ConfigPermissionMgr::new()
                .with_hook_chains(hook_chains.clone())
                .with_seed_rules(default_permission_rules())
                .context("seeding default permission rules")?,
        ))
        .config(Arc::new(config.clone()))
        .hook_chains(hook_chains.clone())
        .plugin_registry(Arc::new(plugin_registry))
        .runtime(runtime)
        .agents(agents)
        .default_agent_id(default_agent_id)
        .default_interaction_mode(default_interaction_mode)
        .workspace_root(workspace_root)
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

    // Claim durable background completions ready for delivery. Leased rows
    // remain owned until their heartbeat stops and their TTL expires.
    leti_server::recover_background_task_deliveries(&state)
        .await
        .context("recovering pending background task deliveries")?;

    // Retry pending or expired deliveries. Each live parent turn renews only
    // its own lease, so one server cannot extend an abandoned worker's row.
    let delivery_reconciler_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            interval.tick().await;
            if let Err(error) =
                leti_server::recover_background_task_deliveries(&delivery_reconciler_state).await
            {
                tracing::error!(error = %error, "failed to reconcile background task deliveries");
            }
        }
    });

    // Resolve the inbound authenticator from the runtime profile. Local
    // profile → dev authenticator (admits a fixed principal, no auth
    // server). Cloud profile → fail-closed: leti-ai ships no real
    // verifier, so boot refuses to start (the cloud binary builds its own
    // and calls `RouterBuilder::build_with_auth`). The mounted AuthLayer
    // now injects the principal the `ask_user` question route requires —
    // no post-hoc Extension layer needed.
    let authenticator = leti_server::auth::authenticator_for_profile()?;
    let authenticator_is_dev = authenticator.is_dev();
    let app = RouterBuilder::default().build_with_auth(state.clone(), authenticator);
    let listener = TcpListener::bind(&config.bind_addr)
        .await
        .with_context(|| format!("binding {}", config.bind_addr))?;
    let local_addr = listener.local_addr().ok();

    // Fail-closed guard on the resolved listener address (see
    // `boot::assert_bind_safe`).
    if let Some(addr) = local_addr {
        leti_server::boot::assert_bind_safe(addr, authenticator_is_dev)?;
    }

    // Spawn the metrics scrape endpoint on its own listener (separate
    // bind, never the public app router) when configured. Detached — the
    // process exits via the main server's graceful shutdown.
    if let Some((bind, handle)) = metrics_setup {
        tokio::spawn(async move {
            if let Err(e) = leti_server::metrics::serve_metrics(bind, handle).await {
                tracing::warn!(error = %e, "metrics endpoint stopped");
            }
        });
    }

    let serve_result = axum::serve(listener, app)
        .with_graceful_shutdown(leti_server::shutdown::shutdown_signal())
        .await
        .context("serving axum");

    // Drain in-flight turn drivers before plugin shutdown.
    leti_server::shutdown::drain_in_flight_turns(
        &state.active_turns,
        std::time::Duration::from_secs(25),
    )
    .await;

    // Drive Plugin::shutdown after axum returns.
    leti_server::shutdown::shutdown_plugins(
        &state.plugin_registry,
        std::time::Duration::from_secs(5),
    )
    .await;

    serve_result?;
    info!("leti-server stopped");
    Ok(())
}
