//! Test harness — boot a fully-wired axum router with in-memory SQLite
//! and stub adapters. Re-used by every integration test in this crate.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use dashmap::DashMap;
use openlet_adapters::bus::BroadcastBus;
use openlet_adapters::config_perm::ConfigPermissionMgr;
use openlet_adapters::localfs::{LocalFilesystem, LocalFsArtifactStore};
use openlet_adapters::localshell::{LocalShellExecutor, LocalShellToolExecutor};
use openlet_adapters::sqlite::event_repo::SqliteEventRepo;
use openlet_adapters::sqlite::{SqliteMemoryStore, open_in_memory};
use openlet_core::adapters::ModelProvider;
use openlet_core::adapters::model_provider::{ChatRequest, ChatStream, ModelPricing};
use openlet_core::config::{Config, LogFormat, PluginsConfig};
use openlet_core::error::ProviderError;
use openlet_core::runtime::{ConversationRuntime, RuntimeConfig};
use openlet_core::tools::builtins::default_registry;
use openlet_core::types::agent::{AgentId, AgentSpec};
use openlet_plugin_registry::PluginRegistry;
use rust_decimal::Decimal;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

/// Test fixture wrapping a router + tempdir handle that survives the
/// test (workspace dir, sqlite-backed artifacts root).
pub struct TestHarness {
    pub router: Router,
    pub events: Arc<dyn openlet_core::adapters::EventSink>,
    pub memory: Arc<dyn openlet_core::adapters::MemoryStore>,
    _tempdir: TempDir,
}

impl TestHarness {
    pub async fn new() -> Self {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let workspace_root = tempdir.path().join("ws");
        let artifact_root = tempdir.path().join("artifacts");
        tokio::fs::create_dir_all(&workspace_root).await.unwrap();
        tokio::fs::create_dir_all(&artifact_root).await.unwrap();

        let pool = open_in_memory().await.expect("sqlite");
        let event_repo = SqliteEventRepo::new(pool.clone());
        let memory: Arc<dyn openlet_core::adapters::MemoryStore> =
            Arc::new(SqliteMemoryStore::new(pool.clone()));
        let events: Arc<dyn openlet_core::adapters::EventSink> =
            Arc::new(BroadcastBus::with_repo(event_repo));
        let memory_handle = memory.clone();
        let events_handle = events.clone();

        let provider: Arc<dyn ModelProvider> = Arc::new(StubProvider);

        let mut config = Config {
            bind_addr: "127.0.0.1:0".to_string(),
            data_dir: tempdir.path().to_path_buf(),
            openrouter_api_key: None,
            default_model: "stub-model".to_string(),
            permission_ruleset_path: None,
            max_cost_per_session_usd: Decimal::new(5, 0),
            log_format: LogFormat::Pretty,
            plugins: PluginsConfig::default(),
        };
        config.data_dir = tempdir.path().to_path_buf();

        let runtime = Arc::new(ConversationRuntime::new(
            provider.clone(),
            memory.clone(),
            events.clone(),
            RuntimeConfig::new(Decimal::new(5, 0), "stub-model".to_string()),
        ));

        let shell_exec = Arc::new(LocalShellExecutor::new(workspace_root.clone()));
        let fs_adapter = Arc::new(LocalFilesystem::new(workspace_root.clone()));
        let shell: Arc<dyn openlet_core::tools::builtins::bash::ShellExecutor> = shell_exec.clone();
        let tool_registry = default_registry(shell.clone());

        let default_agent_id = AgentId::new();
        let agent_spec = AgentSpec::new(default_agent_id, workspace_root.clone(), "default");
        let mut agents: HashMap<AgentId, openlet_server::AgentResources> = HashMap::new();
        agents.insert(
            default_agent_id,
            openlet_server::AgentResources {
                spec: agent_spec,
                fs: fs_adapter.clone(),
                shell: shell.clone(),
            },
        );

        let state = openlet_server::AppState {
            provider,
            memory,
            artifacts: Arc::new(LocalFsArtifactStore::new(artifact_root, pool.clone())),
            tools: Arc::new(LocalShellToolExecutor::new()),
            tool_registry,
            read_histories: Arc::new(DashMap::new()),
            events,
            permission: Arc::new(ConfigPermissionMgr::new()),
            config: Arc::new(config),
            plugin_registry: Arc::new(PluginRegistry::new()),
            runtime,
            active_turns: Arc::new(DashMap::new()),
            agents: Arc::new(agents),
            default_agent_id,
        };

        let router = openlet_server::build_router(state);

        Self {
            router,
            events: events_handle,
            memory: memory_handle,
            _tempdir: tempdir,
        }
    }

    pub fn router(&self) -> Router {
        self.router.clone()
    }
}

pub struct AgentResourcesBag;

/// Provider stub that errors on every call — runtime tests for SSE
/// and cancel don't actually need real LLM output to verify the wire.
struct StubProvider;

#[async_trait]
impl ModelProvider for StubProvider {
    async fn chat_stream(
        &self,
        _req: ChatRequest,
        _cancel: CancellationToken,
    ) -> Result<ChatStream, ProviderError> {
        Err(ProviderError::Unimplemented)
    }

    fn pricing(&self, _model: &str) -> Option<ModelPricing> {
        None
    }
}
