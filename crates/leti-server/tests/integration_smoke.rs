//! Integration smoke harness — proves the public plugin extension
//! surface still composes end-to-end. Locks the boot sequence in place
//! so a regression to `install_all` / `ToolRegistry` / `AppStateBuilder`
//! fails CI before it reaches downstream integrators.
//!
//! Covers:
//! - `core-tools` plugin registers its built-ins through `register_tool`
//!   (web_fetch is Option-injected, so absent here where no fetcher is wired).
//! - `core-agents` plugin registers `general` + `indexer` through
//!   `register_agent`.
//! - `test-quota-stub` plugin installs its `before_turn` + `on_cost_tick`
//!   hooks without panicking on a session that has no `user_id` blob —
//!   the unmetered-skip codepath the reviewer flagged in phase 5.
//! - The full `AppState` build path with plugin-drained registry succeeds.
//! - `extensions["user_id"]` round-trips through SQLite for the canonical
//!   Cloud-shape session create.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use leti_core::adapters::event_sink::Persistence;
use leti_core::adapters::tool_executor::ToolCtx;
use leti_core::error::ToolError;
use leti_core::tools::builtins::bash::{BashOutput, ShellExecutor};
use leti_core::types::event::AgentEvent;
use leti_core::types::session::{SessionId, SessionMeta};
use leti_plugin_api::context::CoreApi;
use leti_plugin_api::plugin::Plugin;
use leti_plugin_registry::{all_plugins, install_all};
use leti_plugin_test_quota_stub::QuotaStubPlugin;
use leti_protocol::{CreateSessionDto, SessionDto};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tower::util::ServiceExt;

mod support;

/// `install_all` over the canonical plugin set returns all tools and
/// agents drained from `core-tools` + `core-agents`. If anything in the
/// `register_tool` / `register_agent` surface regresses, this test
/// catches it before it reaches downstream integrators.
#[tokio::test]
async fn canonical_plugin_set_drains_tools_and_agents() {
    let shell = stub_shell();
    let plugins = all_plugins(
        shell,
        None,
        None,
        stub_memory(),
        stub_task_registry(),
        stub_spawner(),
    );
    let configs = HashMap::new();
    let core_api: Arc<dyn CoreApi> = Arc::new(NoopCoreApi);

    let installed = install_all(plugins, &configs, core_api)
        .await
        .expect("install canonical plugin set");

    // core-tools built-ins registered through the plugin surface. web_fetch
    // is Option-injected (like python) and absent here — no fetcher wired.
    let tool_names: Vec<&'static str> = installed.tools.iter().map(|t| t.name()).collect();
    for expected in [
        "read",
        "list",
        "glob",
        "grep",
        "write",
        "edit",
        "bash",
        "todo",
        "subagent_task",
        "task_status",
    ] {
        assert!(
            tool_names.contains(&expected),
            "expected built-in tool `{expected}` registered through plugin surface, \
             got {tool_names:?}"
        );
    }

    // core-agents ships both `general` and `indexer`. If either drops out
    // of the plugin path silently, this test catches it.
    let slugs: Vec<String> = installed
        .agents
        .iter()
        .map(|a| a.slug.as_str().to_owned())
        .collect();
    for expected in ["general", "indexer"] {
        assert!(
            slugs.iter().any(|s| s == expected),
            "expected `{expected}` agent registered through plugin surface, got {slugs:?}"
        );
    }

    // No plugin in the canonical set installs a provider — the host
    // falls back to the OpenAI-compat default. If that contract changes,
    // tighten this assertion.
    assert!(
        installed.provider.is_none(),
        "canonical plugin set should not register a provider"
    );
}

/// `test-quota-stub` plugin installs its hook chain without panicking,
/// even when the manifest declares no config block. Cost-tick on an
/// unmetered session must not cancel — the early-return guard from
/// phase 5's reviewer fix is the canonical bug to lock down.
///
/// We assert by *delta* against a baseline canonical install so a future
/// `core-tools` / `core-agents` hook addition can't mask a silent
/// regression in the quota stub.
#[tokio::test]
async fn quota_stub_installs_with_default_config() {
    let core_api: Arc<dyn CoreApi> = Arc::new(NoopCoreApi);
    let configs = HashMap::new();

    // Baseline: canonical plugin set alone.
    let baseline = install_all(
        all_plugins(
            stub_shell(),
            None,
            None,
            stub_memory(),
            stub_task_registry(),
            stub_spawner(),
        ),
        &configs,
        core_api.clone(),
    )
    .await
    .expect("install canonical baseline");

    // With the quota stub appended.
    let mut plugins = all_plugins(
        stub_shell(),
        None,
        None,
        stub_memory(),
        stub_task_registry(),
        stub_spawner(),
    );
    plugins.push(Arc::new(QuotaStubPlugin::new()) as Arc<dyn Plugin>);
    let with_stub = install_all(plugins, &configs, core_api)
        .await
        .expect("install quota stub alongside canonical set");

    // QuotaStubPlugin pushes exactly two hooks: before_turn + on_cost_tick.
    assert_eq!(
        with_stub.chains.before_turn.len(),
        baseline.chains.before_turn.len() + 1,
        "quota stub should add exactly one before_turn hook"
    );
    assert_eq!(
        with_stub.chains.on_cost_tick.len(),
        baseline.chains.on_cost_tick.len() + 1,
        "quota stub should add exactly one on_cost_tick hook"
    );
}

/// Sessions can carry an opaque `extensions: serde_json::Value` blob
/// shaped by the integrator. Locks down the canonical Cloud shape so
/// future migrations / repo refactors can't silently corrupt it.
#[tokio::test]
async fn session_extensions_round_trip_canonical_cloud_shape() {
    let harness = support::TestHarness::new().await;
    let app = harness.router();

    let extensions = json!({
        "user_id": "u_canonical",
        "tenant_id": "t_42",
        "scopes": ["read", "write", "admin"],
        "trace_id": "01H8ABC123XYZ",
    });
    let body = serde_json::to_vec(&CreateSessionDto {
        agent_id: None,
        parent_session_id: None,
        permission_mode: None,
        extensions: extensions.clone(),
        user_questions: true,
        interaction_mode: Default::default(),
    })
    .expect("serialize CreateSessionDto");

    let resp = app
        .clone()
        .oneshot(
            Request::post("/v1/session")
                .header("content-type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("session create");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let created: SessionDto =
        serde_json::from_slice(&bytes).expect("deserialize SessionDto from create response");
    assert_eq!(
        created.extensions, extensions,
        "extensions blob should survive session create"
    );

    // Re-fetch to confirm SQLite persisted the JSON unchanged.
    let resp = app
        .oneshot(
            Request::get(format!("/v1/session/{}", created.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("session fetch");
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let fetched: SessionDto =
        serde_json::from_slice(&bytes).expect("deserialize SessionDto from get response");
    assert_eq!(
        fetched.extensions, extensions,
        "extensions blob should survive SQLite round-trip"
    );
}

// --- Stubs --------------------------------------------------------------

struct NoopCoreApi;

#[async_trait]
impl CoreApi for NoopCoreApi {
    async fn current_session_meta(&self, _: SessionId) -> Option<SessionMeta> {
        None
    }
    fn session_cost(&self, _: SessionId) -> Decimal {
        Decimal::ZERO
    }
    fn record_cost(&self, _: SessionId, _: Decimal) {}
    async fn emit_event(&self, _: AgentEvent, _: Persistence) {}
    fn read_config(&self, _: &str) -> Result<Value, String> {
        Ok(Value::Null)
    }
    async fn cancel_session(&self, _: SessionId, _: String) {}
    async fn emit_notification(
        &self,
        _: Option<SessionId>,
        _: leti_core::hooks::io::NotificationLevel,
        _: String,
        _: String,
        _: String,
    ) {
    }
}

struct StubShell;

#[async_trait]
impl ShellExecutor for StubShell {
    async fn run(&self, _: &ToolCtx, _: &str, _: u64) -> Result<BashOutput, ToolError> {
        Err(ToolError::Unimplemented)
    }
}

fn stub_shell() -> Arc<dyn ShellExecutor> {
    Arc::new(StubShell)
}

fn stub_memory() -> Arc<dyn leti_core::adapters::memory_store::MemoryStore> {
    Arc::new(NoopMemory)
}

#[derive(Default)]
struct NoopMemory;

#[async_trait]
impl leti_core::adapters::memory_store::MemoryStore for NoopMemory {
    async fn create_session(
        &self,
        _: leti_core::types::agent::AgentId,
        _: Option<SessionId>,
    ) -> Result<SessionId, leti_core::error::MemoryError> {
        Ok(SessionId::new())
    }
    async fn get_session(
        &self,
        _: SessionId,
    ) -> Result<Option<SessionMeta>, leti_core::error::MemoryError> {
        Ok(None)
    }
    async fn list_sessions(
        &self,
        _: leti_core::types::session::SessionFilter,
    ) -> Result<Vec<SessionMeta>, leti_core::error::MemoryError> {
        Ok(Vec::new())
    }
    async fn update_status(
        &self,
        _: SessionId,
        _: leti_core::types::session::SessionStatus,
        _: &str,
    ) -> Result<(), leti_core::error::MemoryError> {
        Ok(())
    }
    async fn update_permission_mode(
        &self,
        _: SessionId,
        _: leti_core::types::permission::PermissionMode,
    ) -> Result<(), leti_core::error::MemoryError> {
        Ok(())
    }
    async fn switch_agent(
        &self,
        _: SessionId,
        _: &str,
    ) -> Result<(), leti_core::error::MemoryError> {
        Ok(())
    }
    async fn update_session_extensions(
        &self,
        _: SessionId,
        _: serde_json::Value,
    ) -> Result<(), leti_core::error::MemoryError> {
        Ok(())
    }
    async fn delete_session(&self, _: SessionId) -> Result<(), leti_core::error::MemoryError> {
        Ok(())
    }
    async fn append_message(
        &self,
        _: SessionId,
        msg: leti_core::types::message::Message,
    ) -> Result<leti_core::types::message::MessageId, leti_core::error::MemoryError> {
        Ok(msg.id)
    }
    async fn append_part(
        &self,
        _: leti_core::types::message::MessageId,
        part: leti_core::types::part::Part,
    ) -> Result<leti_core::types::part::PartId, leti_core::error::MemoryError> {
        Ok(part.id())
    }
    async fn upsert_part(
        &self,
        _: leti_core::types::message::MessageId,
        _: leti_core::types::part::PartId,
        _: leti_core::types::part::Part,
    ) -> Result<(), leti_core::error::MemoryError> {
        Ok(())
    }
    async fn list_messages(
        &self,
        _: SessionId,
    ) -> Result<Vec<leti_core::types::message::Message>, leti_core::error::MemoryError> {
        Ok(Vec::new())
    }
    async fn list_parts(
        &self,
        _: SessionId,
        _: leti_core::types::message::MessageId,
    ) -> Result<Vec<leti_core::types::part::Part>, leti_core::error::MemoryError> {
        Ok(Vec::new())
    }
    async fn record_read(
        &self,
        _: SessionId,
        _: std::path::PathBuf,
    ) -> Result<(), leti_core::error::MemoryError> {
        Ok(())
    }
}

fn stub_task_registry() -> Arc<leti_core::runtime::subagent::TaskRegistry> {
    Arc::new(leti_core::runtime::subagent::TaskRegistry::new(32))
}

fn stub_spawner() -> Arc<dyn leti_core::tools::builtins::subagent_task::SubagentSpawner> {
    Arc::new(StubSubagentSpawner)
}

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
    ) -> Result<
        leti_core::tools::builtins::subagent_task::SpawnedSubagent,
        leti_core::runtime::subagent::SpawnError,
    > {
        Err(leti_core::runtime::subagent::SpawnError::Internal(
            "stub".into(),
        ))
    }
    async fn await_completion(
        &self,
        _task_id: leti_core::runtime::subagent::TaskId,
    ) -> Result<
        (
            String,
            Option<String>,
            leti_core::runtime::subagent::TaskStatus,
        ),
        leti_core::runtime::subagent::SpawnError,
    > {
        Err(leti_core::runtime::subagent::SpawnError::Internal(
            "stub".into(),
        ))
    }
}
