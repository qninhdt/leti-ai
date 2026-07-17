//! Test harness — boot a fully-wired axum router with in-memory SQLite
//! and stub adapters. Re-used by every integration test in this crate.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use leti_adapters::bus::BroadcastBus;
use leti_adapters::config_perm::ConfigPermissionMgr;
use leti_adapters::localfs::{LocalFilesystem, LocalFsArtifactStore};
use leti_adapters::localshell::LocalShellExecutor;
use leti_adapters::sqlite::event_repo::SqliteEventRepo;
use leti_adapters::sqlite::{SqliteMemoryStore, open_in_memory};
use leti_core::adapters::ModelProvider;
use leti_core::adapters::model_provider::{ChatRequest, ChatStream, ModelPricing};
use leti_core::config::{Config, LogFormat, PluginsConfig};
use leti_core::error::ProviderError;
use leti_core::runtime::subagent::{SpawnError, TaskId, TaskStatus};
use leti_core::runtime::{ConversationRuntime, RuntimeConfig};
use leti_core::types::agent::{AgentId, AgentSpec};
use leti_plugin_api::context::CoreApi;
use leti_plugin_registry::{PluginHandles, install_all};
use rust_decimal::Decimal;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

/// Test fixture wrapping a router + tempdir handle that survives the
/// test (workspace dir, sqlite-backed artifacts root).
pub struct TestHarness {
    pub router: Router,
    pub events: Arc<dyn leti_core::adapters::EventSink>,
    pub memory: Arc<dyn leti_core::adapters::MemoryStore>,
    _tempdir: TempDir,
}

impl TestHarness {
    /// Build a wired-up `AppState` plus the tempdir guard it depends on.
    /// Shared by `new()` (which then mounts the default router) and the
    /// `RouterBuilder` composability tests (which mount a subset).
    pub async fn build_state() -> (leti_server::AppState, TempDir) {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let workspace_root = tempdir.path().join("ws");
        let artifact_root = tempdir.path().join("artifacts");
        tokio::fs::create_dir_all(&workspace_root).await.unwrap();
        tokio::fs::create_dir_all(&artifact_root).await.unwrap();

        let pool = open_in_memory().await.expect("sqlite");
        let event_repo = SqliteEventRepo::new(pool.clone());
        let memory: Arc<dyn leti_core::adapters::MemoryStore> =
            Arc::new(SqliteMemoryStore::new(pool.clone()));
        let events: Arc<dyn leti_core::adapters::EventSink> =
            Arc::new(BroadcastBus::with_repo(event_repo));

        let provider: Arc<dyn ModelProvider> = Arc::new(StubProvider);

        let config = Config {
            bind_addr: "127.0.0.1:0".to_string(),
            data_dir: tempdir.path().to_path_buf(),
            default_model: "stub-model".to_string(),
            permission_ruleset_path: None,
            log_format: LogFormat::Pretty,
            plugins: PluginsConfig::default(),
            tool_scheduler: Default::default(),
        };

        let runtime = Arc::new(ConversationRuntime::new(
            provider.clone(),
            memory.clone(),
            events.clone(),
            RuntimeConfig::new("stub-model".to_string()),
        ));

        let shell_exec = Arc::new(LocalShellExecutor::new(workspace_root.clone()));
        let fs_adapter = Arc::new(LocalFilesystem::new(workspace_root.clone()));
        let shell: Arc<dyn leti_core::tools::builtins::bash::ShellExecutor> = shell_exec.clone();

        // Install plugins so the test harness exercises the same
        // tool-registration path as the server binary. Keeps the harness
        // and `main.rs` in lockstep — any drift would surface as a test
        // that passes locally but fails in production wiring.
        let core_api: Arc<dyn CoreApi> = Arc::new(NoopCoreApi);
        let task_registry = Arc::new(leti_core::runtime::subagent::TaskRegistry::new(32));
        let spawner: Arc<dyn leti_core::tools::builtins::subagent_task::SubagentSpawner> =
            Arc::new(StubSubagentSpawner);
        let plugins = leti_plugin_registry::all_plugins(
            shell.clone(),
            None,
            None,
            memory.clone(),
            task_registry.clone(),
            spawner,
        );
        let configs = std::collections::HashMap::new();
        let installed = install_all(plugins, &configs, core_api)
            .await
            .expect("install plugins");

        let mut tool_builder = leti_core::tools::ToolRegistry::builder();
        for tool in installed.tools {
            tool_builder = tool_builder.register_erased(tool);
        }
        let tool_registry = tool_builder.build();

        let default_agent_id = AgentId::new();
        let agent_spec = AgentSpec::new(default_agent_id, workspace_root.clone(), "default");
        let mut agents: HashMap<AgentId, leti_server::AgentResources> = HashMap::new();
        agents.insert(
            default_agent_id,
            leti_server::AgentResources {
                spec: agent_spec,
                fs: fs_adapter.clone(),
                shell: shell.clone(),
            },
        );

        let state = leti_server::AppStateBuilder::new()
            .provider(provider)
            .memory(memory)
            .artifacts(Arc::new(LocalFsArtifactStore::new(
                artifact_root,
                pool.clone(),
            )))
            .tool_registry(tool_registry)
            .events(events)
            .permission(Arc::new(ConfigPermissionMgr::new()))
            .config(Arc::new(config))
            .plugin_registry(Arc::new(PluginHandles::new()))
            .runtime(runtime)
            .agents(agents)
            .default_agent_id(default_agent_id)
            .workspace_root(workspace_root.clone())
            .agent_registry(Arc::new(leti_core::agent::AgentRegistry::new()))
            .build()
            .expect("build app state");

        (state, tempdir)
    }

    /// Just the wired-up `AppState`. The tempdir's drop guard is
    /// released so paths inside `state` stay valid for the test process.
    /// Cheap because tests are short-lived.
    pub async fn raw_state() -> leti_server::AppState {
        let (state, tempdir) = Self::build_state().await;
        let _ = tempdir.keep();
        state
    }
    pub async fn new() -> Self {
        let (state, tempdir) = Self::build_state().await;
        let memory_handle = state.memory.clone();
        let events_handle = state.events.clone();
        let router = leti_server::build_router(state);

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

/// Minimal `CoreApi` impl for the test harness. Plugin install needs an
/// `Arc<dyn CoreApi>` so closures can capture it; tests don't drive any
/// hook code paths that read these methods, so noop is sufficient.
struct NoopCoreApi;

#[async_trait]
impl CoreApi for NoopCoreApi {
    async fn current_session_meta(
        &self,
        _: leti_core::types::session::SessionId,
    ) -> Option<leti_core::types::session::SessionMeta> {
        None
    }
    fn session_cost(&self, _: leti_core::types::session::SessionId) -> Decimal {
        Decimal::ZERO
    }
    fn record_cost(&self, _: leti_core::types::session::SessionId, _: Decimal) {}
    async fn emit_event(
        &self,
        _: leti_core::types::event::AgentEvent,
        _: leti_core::adapters::event_sink::Persistence,
    ) {
    }
    fn read_config(&self, _: &str) -> Result<serde_json::Value, String> {
        Ok(serde_json::Value::Null)
    }
    async fn cancel_session(&self, _: leti_core::types::session::SessionId, _: String) {}
    async fn emit_notification(
        &self,
        _: Option<leti_core::types::session::SessionId>,
        _: leti_core::hooks::io::NotificationLevel,
        _: String,
        _: String,
        _: String,
    ) {
    }
}

/// Test stub for `SubagentSpawner` — every call returns
/// `SpawnError::Internal` so integration tests that don't exercise
/// nested subagents can still install `core-tools` cleanly.
struct StubSubagentSpawner;

#[async_trait]
impl leti_core::tools::builtins::subagent_task::SubagentSpawner for StubSubagentSpawner {
    async fn spawn(
        &self,
        _ctx: &leti_core::adapters::tool_executor::ToolCtx,
        _subagent_type: &str,
        _objective: &str,
        _scope: Option<&str>,
        _background: bool,
    ) -> Result<leti_core::tools::builtins::subagent_task::SpawnedSubagent, SpawnError> {
        Err(SpawnError::Internal("stub spawner".into()))
    }
    async fn await_completion(
        &self,
        _task_id: TaskId,
    ) -> Result<(String, Option<String>, TaskStatus), SpawnError> {
        Err(SpawnError::Internal("stub spawner".into()))
    }
}

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
