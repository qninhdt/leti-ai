//! Shared adapter construction — sqlite pool, directories, base adapters.
//!
//! Both `run_server` and `doctor_cmd::build_doctor_state` wire the same
//! sqlite pool + artifact dir + provider + memory + events + workspace.
//! This module extracts that shared setup into a single struct so the two
//! boot paths stay in sync without duplication.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use openlet_adapters::bus::BroadcastBus;
use openlet_adapters::localfs::{LocalFilesystem, LocalFsArtifactStore};
use openlet_adapters::localshell::LocalShellExecutor;
use openlet_adapters::sqlite::SqliteMemoryStore;
use openlet_core::config::Config;

/// Pre-built adapters shared by both the serve and doctor paths.
pub struct AdapterStack {
    pub artifacts: Arc<dyn openlet_core::adapters::ArtifactStore>,
    pub provider: Arc<dyn openlet_core::adapters::ModelProvider>,
    pub memory: Arc<dyn openlet_core::adapters::MemoryStore>,
    pub events: Arc<dyn openlet_core::adapters::EventSink>,
    pub workspace_root: PathBuf,
    pub fs: Arc<LocalFilesystem>,
    pub shell: Arc<dyn openlet_core::tools::builtins::bash::ShellExecutor>,
}

/// Configuration for building the adapter stack.
pub struct AdapterStackConfig<'a> {
    pub config: &'a Config,
    /// Pre-resolved model provider (built by caller from env + config).
    pub provider: Arc<dyn openlet_core::adapters::ModelProvider>,
    /// Pre-resolved workspace root path.
    pub workspace_root: PathBuf,
    /// Max sqlite pool connections. `run_server` uses 8; `doctor` uses 2.
    pub pool_size: u32,
    /// Whether to create missing directories with strict error handling
    /// (true for serve, false/lenient for doctor).
    pub strict_dirs: bool,
}

impl AdapterStack {
    /// Build the adapter stack from config. Opens the sqlite pool, runs
    /// migrations, creates directories, and wires the base adapters.
    pub async fn build(opts: AdapterStackConfig<'_>) -> anyhow::Result<Self> {
        let config = opts.config;
        let db_path = config.data_dir.join("db.sqlite");
        let artifact_root = config.data_dir.join("artifacts");

        let pool = openlet_adapters::sqlite::open_pool(&db_path, opts.pool_size)
            .await
            .with_context(|| format!("opening sqlite at {}", db_path.display()))?;
        openlet_adapters::sqlite::run_migrations(&pool)
            .await
            .context("running sqlite migrations")?;

        if opts.strict_dirs {
            tokio::fs::create_dir_all(&artifact_root)
                .await
                .with_context(|| format!("create artifact dir {}", artifact_root.display()))?;
        } else {
            tokio::fs::create_dir_all(&artifact_root).await.ok();
        }

        let memory: Arc<dyn openlet_core::adapters::MemoryStore> =
            Arc::new(SqliteMemoryStore::new(pool.clone()));
        let event_repo = openlet_adapters::sqlite::event_repo::SqliteEventRepo::new(pool.clone());
        let events: Arc<dyn openlet_core::adapters::EventSink> =
            Arc::new(BroadcastBus::with_repo(event_repo));
        let artifacts: Arc<dyn openlet_core::adapters::ArtifactStore> =
            Arc::new(LocalFsArtifactStore::new(artifact_root, pool));

        let workspace_root = opts.workspace_root;
        if opts.strict_dirs {
            tokio::fs::create_dir_all(&workspace_root)
                .await
                .with_context(|| format!("create workspace dir {}", workspace_root.display()))?;
        } else {
            tokio::fs::create_dir_all(&workspace_root).await.ok();
        }

        let shell_exec = Arc::new(LocalShellExecutor::new(workspace_root.clone()));
        let fs = Arc::new(LocalFilesystem::new(workspace_root.clone()));
        let shell: Arc<dyn openlet_core::tools::builtins::bash::ShellExecutor> = shell_exec.clone();

        Ok(Self {
            artifacts,
            provider: opts.provider,
            memory,
            events,
            workspace_root,
            fs,
            shell,
        })
    }
}
