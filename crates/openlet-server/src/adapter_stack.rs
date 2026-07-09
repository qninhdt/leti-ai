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
use openlet_adapters::cloudfs::CloudFilesystem;
use openlet_adapters::localfs::{LocalFilesystem, LocalFsArtifactStore};
use openlet_adapters::emushell::EmulatedShellExecutor;
use openlet_adapters::localshell::LocalShellExecutor;
use openlet_adapters::pyexec::MontyExecutor;
use openlet_adapters::sqlite::SqliteMemoryStore;
use openlet_core::adapters::Filesystem;
use openlet_core::config::Config;
use secrecy::ExposeSecret;

/// Which `ShellExecutor` impl the `bash` tool dispatches into. Selected at
/// boot from `OPENLET_SHELL_IMPL` so a production regression in the emulated
/// interpreter can be rolled back to the legacy subprocess executor by
/// flipping one env var + restarting — no redeploy, no code change (plan
/// Phase 7 / finding FM7: "no runtime rollback").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellImpl {
    /// In-process `EmulatedShellExecutor` — the production default. No
    /// subprocess, no host env/network; every file op routes through `ctx.fs`,
    /// so it is the only impl that works in cloud mode.
    Emulated,
    /// Legacy subprocess `LocalShellExecutor` (`bash -c`). Kept behind the flag
    /// for at least one release cycle as a rollback lever. Runs real bash
    /// against a LOCAL workspace dir, so it bypasses `ctx.fs` entirely and is
    /// incompatible with cloud mode.
    Subprocess,
}

/// Parse the `OPENLET_SHELL_IMPL` value into a [`ShellImpl`]. Pure so it can be
/// unit-tested without mutating process env. Unset / empty → `Emulated` (the
/// default). Unknown values are a hard error rather than a silent fallback so a
/// typo (`subproces`) can't quietly ship the wrong executor.
///
/// Accepted (case-insensitive, trimmed): `emulated`, `subprocess`.
fn parse_shell_impl(raw: Option<&str>) -> Result<ShellImpl, String> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(ShellImpl::Emulated),
        Some(v) => match v.to_ascii_lowercase().as_str() {
            "emulated" => Ok(ShellImpl::Emulated),
            "subprocess" => Ok(ShellImpl::Subprocess),
            other => Err(format!(
                "OPENLET_SHELL_IMPL='{other}' is not recognized (expected 'emulated' or 'subprocess')"
            )),
        },
    }
}

/// Resolve the shell impl from the raw env value AND the cloud-mode flag,
/// folding parsing + the subprocess/cloud compatibility guard into one pure
/// function so the whole decision — not just the string parse — is unit-testable
/// without mutating process env or opening a sqlite pool (`build()` requires
/// both). `build()` calls this and only then constructs the chosen executor.
///
/// `cloud_fs_enabled` is `config.cloud_fs.is_some()`. The subprocess executor
/// runs real `bash` against a local workspace dir and never touches `ctx.fs`,
/// so pairing it with cloud mode would silently operate on the wrong (empty
/// local) filesystem — that combination is a hard error.
fn resolve_shell_impl(raw: Option<&str>, cloud_fs_enabled: bool) -> Result<ShellImpl, String> {
    let impl_ = parse_shell_impl(raw)?;
    if impl_ == ShellImpl::Subprocess && cloud_fs_enabled {
        return Err(
            "OPENLET_SHELL_IMPL=subprocess is incompatible with cloud filesystem mode \
             (LocalShellExecutor bypasses ctx.fs and would operate on the local disk, \
             not the cloud workspace). Use the emulated shell in cloud mode."
                .to_string(),
        );
    }
    Ok(impl_)
}

/// Pre-built adapters shared by both the serve and doctor paths.
pub struct AdapterStack {
    pub artifacts: Arc<dyn openlet_core::adapters::ArtifactStore>,
    pub provider: Arc<dyn openlet_core::adapters::ModelProvider>,
    pub memory: Arc<dyn openlet_core::adapters::MemoryStore>,
    pub events: Arc<dyn openlet_core::adapters::EventSink>,
    pub workspace_root: PathBuf,
    /// Injected `Filesystem` — `LocalFilesystem` in local mode, or
    /// `CloudFilesystem` (file-service gRPC) when `config.cloud_fs` is set.
    /// The emulated bash/python executors are identical either way; only this
    /// impl changes (the whole point of Phase 6).
    pub fs: Arc<dyn Filesystem>,
    pub shell: Arc<dyn openlet_core::tools::builtins::bash::ShellExecutor>,
    /// In-process Python executor (Monty) for the `python` tool. Same
    /// security-by-construction story as `shell`: no subprocess, no host env,
    /// every file op routed through `ctx.fs`.
    pub python: Arc<dyn openlet_core::tools::builtins::python::PythonExecutor>,
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

        // Emulated in-process shell: no subprocess, no host env/network. All
        // file ops route through `ctx.fs`, so the same executor works against
        // the local FS or a cloud backend. `LocalShellExecutor` is kept in the
        // crate until the cutover phase as a runtime-selectable fallback.
        // Cloud mode swaps ONLY the Filesystem impl; the emulated shell/python
        // are identical either way (they hold `Arc<dyn Filesystem>`). Cloud
        // mode is opt-in via config (deploy-ordering contract: OFF until
        // file-service ships the GrepFiles RPC).
        let fs: Arc<dyn Filesystem> = match config.cloud_fs.as_ref() {
            Some(cloud) => {
                let channel = tonic::transport::Channel::from_shared(cloud.endpoint.clone())
                    .with_context(|| format!("invalid cloud fs endpoint {}", cloud.endpoint))?
                    .connect_lazy();
                Arc::new(CloudFilesystem::new(
                    channel,
                    cloud.workspace_id.clone(),
                    cloud.bearer.expose_secret().to_string(),
                ))
            }
            None => Arc::new(LocalFilesystem::new(workspace_root.clone())),
        };

        // Runtime-selectable shell impl (rollback lever — see `ShellImpl`).
        // Default is the emulated in-process interpreter; `subprocess` restores
        // the legacy `LocalShellExecutor`. `resolve_shell_impl` also enforces
        // the subprocess/cloud incompatibility guard. Read the raw value with
        // `var_os` so a non-UTF-8 value surfaces as a parse error rather than
        // silently defaulting (`var().ok()` would swallow `NotUnicode`).
        let shell_impl_raw = std::env::var_os("OPENLET_SHELL_IMPL");
        let shell_impl_str = match shell_impl_raw.as_ref() {
            Some(os) => Some(os.to_str().ok_or_else(|| {
                anyhow::anyhow!("OPENLET_SHELL_IMPL contains non-UTF-8 bytes")
            })?),
            None => None,
        };
        let shell_impl = resolve_shell_impl(shell_impl_str, config.cloud_fs.is_some())
            .map_err(|e| anyhow::anyhow!(e))?;
        let shell: Arc<dyn openlet_core::tools::builtins::bash::ShellExecutor> = match shell_impl {
            ShellImpl::Emulated => Arc::new(EmulatedShellExecutor::new()),
            ShellImpl::Subprocess => {
                tracing::warn!(
                    "OPENLET_SHELL_IMPL=subprocess: using the legacy subprocess bash executor \
                     (rollback mode). This runs real bash on the host and is NOT cloud-safe."
                );
                Arc::new(LocalShellExecutor::new(workspace_root.clone()))
            }
        };
        let python: Arc<dyn openlet_core::tools::builtins::python::PythonExecutor> =
            Arc::new(MontyExecutor::new());

        Ok(Self {
            artifacts,
            provider: opts.provider,
            memory,
            events,
            workspace_root,
            fs,
            shell,
            python,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{ShellImpl, parse_shell_impl, resolve_shell_impl};

    #[test]
    fn unset_or_empty_defaults_to_emulated() {
        assert_eq!(parse_shell_impl(None).unwrap(), ShellImpl::Emulated);
        assert_eq!(parse_shell_impl(Some("")).unwrap(), ShellImpl::Emulated);
        assert_eq!(parse_shell_impl(Some("   ")).unwrap(), ShellImpl::Emulated);
    }

    #[test]
    fn recognizes_both_impls_case_insensitively_and_trimmed() {
        assert_eq!(parse_shell_impl(Some("emulated")).unwrap(), ShellImpl::Emulated);
        assert_eq!(parse_shell_impl(Some("EMULATED")).unwrap(), ShellImpl::Emulated);
        assert_eq!(parse_shell_impl(Some(" subprocess ")).unwrap(), ShellImpl::Subprocess);
        assert_eq!(parse_shell_impl(Some("Subprocess")).unwrap(), ShellImpl::Subprocess);
    }

    #[test]
    fn unknown_value_is_hard_error_not_silent_default() {
        // A typo must fail loudly rather than quietly shipping the emulated
        // default — the whole point of the rollback lever is being explicit.
        let err = parse_shell_impl(Some("subproces")).unwrap_err();
        assert!(err.contains("subproces"), "error should echo the bad value: {err}");
        assert!(err.contains("emulated") && err.contains("subprocess"));
    }

    // --- resolve_shell_impl: the actual build() wiring decision (parse +
    // cloud-compat guard), proven without opening a sqlite pool or mutating
    // env. This is the "rollback verify" success criterion at the wiring level,
    // not just the string parser.

    #[test]
    fn resolve_defaults_to_emulated_in_local_mode() {
        // Unset env + local FS → emulated (the production default).
        assert_eq!(resolve_shell_impl(None, false).unwrap(), ShellImpl::Emulated);
    }

    #[test]
    fn resolve_subprocess_selected_in_local_mode() {
        // The rollback lever: OPENLET_SHELL_IMPL=subprocess in local mode
        // resolves to the subprocess executor (build() then constructs
        // LocalShellExecutor for this variant).
        assert_eq!(
            resolve_shell_impl(Some("subprocess"), false).unwrap(),
            ShellImpl::Subprocess
        );
    }

    #[test]
    fn resolve_emulated_allowed_in_cloud_mode() {
        // Emulated is the only cloud-safe impl (it routes through ctx.fs), so
        // cloud mode + emulated must be accepted.
        assert_eq!(
            resolve_shell_impl(Some("emulated"), true).unwrap(),
            ShellImpl::Emulated
        );
        assert_eq!(resolve_shell_impl(None, true).unwrap(), ShellImpl::Emulated);
    }

    #[test]
    fn resolve_subprocess_plus_cloud_is_hard_error() {
        // The subprocess executor bypasses ctx.fs and runs bash on local disk,
        // so pairing it with cloud mode would silently operate on the wrong
        // filesystem. That combination must bail rather than serve a broken
        // agent.
        let err = resolve_shell_impl(Some("subprocess"), true).unwrap_err();
        assert!(
            err.contains("cloud") && err.contains("subprocess"),
            "error should explain the incompatibility: {err}"
        );
    }

    #[test]
    fn resolve_propagates_parse_error() {
        // An unknown value fails at the parse step even before the cloud guard.
        assert!(resolve_shell_impl(Some("nope"), false).is_err());
        assert!(resolve_shell_impl(Some("nope"), true).is_err());
    }
}
