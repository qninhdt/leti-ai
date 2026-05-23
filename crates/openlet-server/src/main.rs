//! Openlet server binary entry point.
//!
//! Bootstrap order: parse CLI → load `Config` → init tracing → build
//! `AppState` with stub adapters → serve axum on `Config::bind_addr` with
//! graceful Ctrl+C shutdown.

mod app_state;
mod cli;
mod openapi;
mod router;
mod routes;

use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use dashmap::DashMap;
use openlet_adapters::{
    bus::BroadcastBus, config_perm::ConfigPermissionMgr, localfs::LocalFsArtifactStore,
    localshell::LocalShellToolExecutor, openai_compat::OpenAiCompatProvider,
    sqlite::SqliteMemoryStore,
};
use openlet_core::config::{Config, LogFormat};
use openlet_plugin_registry::PluginRegistry;
use tokio::net::TcpListener;
use tokio::signal;
use tracing::info;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::app_state::AppState;
use crate::cli::{Cli, Command};

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

    let state = AppState {
        provider: Arc::new(OpenAiCompatProvider::new()),
        memory: Arc::new(SqliteMemoryStore::new(pool.clone())),
        artifacts: Arc::new(LocalFsArtifactStore::new(artifact_root, pool.clone())),
        tools: Arc::new(LocalShellToolExecutor::new()),
        events: Arc::new(BroadcastBus::new()),
        permission: Arc::new(ConfigPermissionMgr::new()),
        config: Arc::new(config.clone()),
        plugin_registry: Arc::new(PluginRegistry::new()),
        active_turns: Arc::new(DashMap::new()),
    };

    let app = router::build(state);
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
