//! Openlet server binary entry point.
//!
//! Bootstrap order: parse CLI → load `Config` → init tracing → build
//! `AppState` with stub adapters → serve axum on `Config::bind_addr` with
//! graceful Ctrl+C shutdown.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use dashmap::DashMap;
use openlet_adapters::{
    bus::BroadcastBus, config_perm::ConfigPermissionMgr,
    localfs::{LocalFilesystem, LocalFsArtifactStore},
    localshell::{LocalShellExecutor, LocalShellToolExecutor},
    openai_compat::OpenAiCompatProvider, sqlite::SqliteMemoryStore,
};
use openlet_core::config::{Config, LogFormat};
use openlet_core::runtime::{ConversationRuntime, RuntimeConfig};
use openlet_core::tools::builtins::default_registry;
use openlet_core::types::agent::{AgentId, AgentSpec};
use openlet_plugin_api::{PluginContext, context::CoreApi};
use openlet_plugin_registry::PluginRegistry;
use openlet_server::{AgentResources, AppState, build_router, cli::{Cli, Command}};
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
        Command::Audit(_) => {
            tracing::warn!("audit subcommand reserved for Phase 8");
            Ok(())
        }
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
    let memory: Arc<dyn openlet_core::adapters::MemoryStore> =
        Arc::new(SqliteMemoryStore::new(pool.clone()));
    let event_repo = openlet_adapters::sqlite::event_repo::SqliteEventRepo::new(pool.clone());
    let events: Arc<dyn openlet_core::adapters::EventSink> =
        Arc::new(BroadcastBus::with_repo(event_repo));

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

    let runtime = Arc::new(ConversationRuntime::new(
        provider.clone(),
        memory.clone(),
        events.clone(),
        RuntimeConfig::new(
            config.max_cost_per_session_usd,
            config.default_model.clone(),
        ),
    ));

    let workspace_root = std::env::var("OPENLET_WORKSPACE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| config.data_dir.join("workspace"));
    tokio::fs::create_dir_all(&workspace_root)
        .await
        .with_context(|| format!("create workspace dir {}", workspace_root.display()))?;

    let shell_exec = Arc::new(LocalShellExecutor::new(workspace_root.clone()));
    let fs_adapter = Arc::new(LocalFilesystem::new(workspace_root.clone()));
    let shell: Arc<dyn openlet_core::tools::builtins::bash::ShellExecutor> = shell_exec.clone();
    let tool_registry = default_registry(shell.clone());

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

    let state = AppState {
        provider,
        memory,
        artifacts: Arc::new(LocalFsArtifactStore::new(artifact_root, pool.clone())),
        tools: Arc::new(LocalShellToolExecutor::new()),
        tool_registry,
        read_histories: Arc::new(DashMap::new()),
        events,
        permission: Arc::new(ConfigPermissionMgr::new()),
        config: Arc::new(config.clone()),
        plugin_registry: Arc::new(PluginRegistry::new()),
        runtime,
        active_turns: Arc::new(DashMap::new()),
        agents: Arc::new(agents),
        default_agent_id,
        agent_registry: Arc::new(install_agents(&config).await?),
    };

    let app = build_router(state);
    let listener = TcpListener::bind(&config.bind_addr)
        .await
        .with_context(|| format!("binding {}", config.bind_addr))?;

    info!(
        bind = %config.bind_addr,
        "bound localhost-only at http://{} (set OPENLET_BIND to expose)",
        config.bind_addr
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serving axum")?;

    info!("openlet-server stopped");
    Ok(())
}

/// Install all compile-time plugins and drain their registered agents into
/// a single `AgentRegistry`. Called once during boot.
async fn install_agents(
    _config: &Config,
) -> anyhow::Result<openlet_core::agent::AgentRegistry> {
    struct StubCore;
    impl CoreApi for StubCore {}

    let mut registry = openlet_core::agent::AgentRegistry::new();
    for plugin in openlet_plugin_registry::all_plugins() {
        let manifest = plugin.manifest().clone();
        let mut ctx = PluginContext::new(
            manifest.clone(),
            serde_json::Value::Null,
            Arc::new(StubCore) as Arc<dyn CoreApi>,
        );
        plugin
            .install(&mut ctx)
            .await
            .with_context(|| format!("installing plugin {}", manifest.id))?;
        for def in ctx.take_registered_agents() {
            registry
                .insert(def)
                .with_context(|| format!("registering agent from {}", manifest.id))?;
        }
    }
    Ok(registry)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("install Ctrl+C handler");
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
