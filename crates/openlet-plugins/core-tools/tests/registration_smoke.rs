//! Registration smoke test for [`CoreToolsPlugin`].
//!
//! The plugin's whole job is to register the built-in tool set through the
//! public `register_tool` extension point. This test installs it against
//! stub dependencies and asserts the contract every boot relies on:
//!   - exactly 15 tools are registered,
//!   - no two share a wire name (a collision would mis-route dispatch),
//!   - install succeeds with no capability error.
//!
//! Stubs only need to satisfy trait bounds — the smoke test never INVOKES a
//! tool, it just drives `install` and inspects the drained registrations.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use openlet_core::adapters::event_sink::Persistence;
use openlet_core::adapters::memory_store::MemoryStore;
use openlet_core::adapters::tool_executor::ToolCtx;
use openlet_core::error::{MemoryError, ToolError};
use openlet_core::runtime::subagent::TaskRegistry;
use openlet_core::runtime::subagent::task_types::{SpawnError, TaskId, TaskStatus};
use openlet_core::tools::builtins::bash::{BashOutput, ShellExecutor};
use openlet_core::tools::builtins::python::{PythonExecutor, PythonOutput};
use openlet_core::tools::builtins::subagent_task::SubagentSpawner;
use openlet_core::types::agent::AgentId;
use openlet_core::types::event::AgentEvent;
use openlet_core::types::message::{Message, MessageId};
use openlet_core::types::part::{Part, PartId};
use openlet_core::types::permission::PermissionMode;
use openlet_core::types::session::{SessionFilter, SessionId, SessionMeta, SessionStatus};
use openlet_plugin_api::Plugin;
use openlet_plugin_api::context::{CoreApi, PluginContext};
use openlet_plugin_api::hooks::io::NotificationLevel;
use openlet_plugin_core_tools::CoreToolsPlugin;

// --- Stub dependencies -----------------------------------------------------

/// Bash executor stub — never called by the smoke test (no tool runs).
struct StubShell;

#[async_trait]
impl ShellExecutor for StubShell {
    async fn run(
        &self,
        _ctx: &ToolCtx,
        _command: &str,
        _timeout_ms: u64,
    ) -> Result<BashOutput, ToolError> {
        Err(ToolError::Io("stub shell".to_string()))
    }
}

/// Python executor stub — never invoked; only needs to type-check so we can
/// prove `.with_python()` registers the `python` tool.
struct StubPython;

#[async_trait]
impl PythonExecutor for StubPython {
    async fn run(
        &self,
        _ctx: &ToolCtx,
        _code: &str,
        _timeout_ms: u64,
    ) -> Result<PythonOutput, ToolError> {
        Err(ToolError::Io("stub python".to_string()))
    }
}

/// Subagent spawner stub — install doesn't spawn, so both methods are dead
/// paths that only need to type-check.
struct StubSpawner;

#[async_trait]
impl SubagentSpawner for StubSpawner {
    async fn spawn(
        &self,
        _ctx: &ToolCtx,
        _subagent_type: &str,
        _objective: &str,
    ) -> Result<TaskId, SpawnError> {
        Err(SpawnError::Internal("stub".to_string()))
    }
    async fn await_completion(
        &self,
        _task_id: TaskId,
    ) -> Result<(String, Option<String>, TaskStatus), SpawnError> {
        Err(SpawnError::Internal("stub".to_string()))
    }
}

/// Minimal in-memory `MemoryStore` — the plan-mode tools hold an `Arc<dyn
/// MemoryStore>` at construction but the smoke test never invokes them, so
/// the bodies are no-ops that satisfy the trait.
#[derive(Default)]
struct StubMemory;

#[async_trait]
impl MemoryStore for StubMemory {
    async fn create_session(
        &self,
        _agent_id: AgentId,
        _parent: Option<SessionId>,
    ) -> Result<SessionId, MemoryError> {
        Ok(SessionId::new())
    }
    async fn get_session(&self, _session: SessionId) -> Result<Option<SessionMeta>, MemoryError> {
        Ok(None)
    }
    async fn list_sessions(&self, _filter: SessionFilter) -> Result<Vec<SessionMeta>, MemoryError> {
        Ok(Vec::new())
    }
    async fn update_status(
        &self,
        _session: SessionId,
        _status: SessionStatus,
        _reason: &str,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn update_permission_mode(
        &self,
        _session: SessionId,
        _mode: PermissionMode,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn switch_agent(
        &self,
        _session: SessionId,
        _agent_slug: &str,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn update_session_extensions(
        &self,
        _session: SessionId,
        _extensions: serde_json::Value,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn delete_session(&self, _session: SessionId) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn append_message(
        &self,
        _session: SessionId,
        msg: Message,
    ) -> Result<MessageId, MemoryError> {
        Ok(msg.id)
    }
    async fn append_part(&self, _msg: MessageId, part: Part) -> Result<PartId, MemoryError> {
        Ok(part.id())
    }
    async fn upsert_part(
        &self,
        _msg: MessageId,
        _pid: PartId,
        _part: Part,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn list_messages(&self, _session: SessionId) -> Result<Vec<Message>, MemoryError> {
        Ok(Vec::new())
    }
    async fn list_parts(
        &self,
        _session: SessionId,
        _msg: MessageId,
    ) -> Result<Vec<Part>, MemoryError> {
        Ok(Vec::new())
    }
    async fn record_read(&self, _session: SessionId, _path: PathBuf) -> Result<(), MemoryError> {
        Ok(())
    }
}

/// No-op `CoreApi` for the `PluginContext` — install never calls back in.
struct NoopCoreApi;

#[async_trait]
impl CoreApi for NoopCoreApi {
    async fn current_session_meta(&self, _: SessionId) -> Option<SessionMeta> {
        None
    }
    fn session_cost(&self, _: SessionId) -> rust_decimal::Decimal {
        rust_decimal::Decimal::ZERO
    }
    fn record_cost(&self, _: SessionId, _: rust_decimal::Decimal) {}
    async fn emit_event(&self, _: AgentEvent, _: Persistence) {}
    fn read_config(&self, _: &str) -> Result<serde_json::Value, String> {
        Ok(serde_json::Value::Null)
    }
    async fn cancel_session(&self, _: SessionId, _: String) {}
    async fn emit_notification(
        &self,
        _: Option<SessionId>,
        _: NotificationLevel,
        _: String,
        _: String,
        _: String,
    ) {
    }
}

fn build_plugin() -> CoreToolsPlugin {
    CoreToolsPlugin::new(
        Arc::new(StubShell),
        Arc::new(StubMemory),
        Arc::new(TaskRegistry::new(8)),
        Arc::new(StubSpawner),
    )
}

#[tokio::test]
async fn installs_all_fifteen_tools_without_collision() {
    let plugin = build_plugin();
    let mut ctx = PluginContext::new(
        plugin.manifest().clone(),
        serde_json::Value::Null,
        Arc::new(NoopCoreApi),
    );

    // Install must succeed: every `register_tool` is gated on
    // `Capability::Tool`, which the manifest declares. A missing capability
    // would surface here as a PluginError.
    plugin
        .install(&mut ctx)
        .await
        .expect("core-tools install must succeed (capability declared)");

    let regs = ctx.into_registrations();
    let names: Vec<&str> = regs.tools.iter().map(|t| t.name()).collect();

    assert_eq!(
        names.len(),
        15,
        "core-tools must register exactly 15 tools, got {names:?}"
    );

    // No id collisions — dedup the names and compare counts.
    let mut unique = names.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        names.len(),
        "tool ids must be unique; duplicates in {names:?}"
    );

    // Spot-check the headline tools are present so a silent rename of the
    // set is caught (not just the count).
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
        "promote_task",
        "send_message",
    ] {
        assert!(
            names.contains(&expected),
            "expected built-in tool '{expected}' to be registered; got {names:?}"
        );
    }

    // The plugin registers no agents or providers — only tools.
    assert!(regs.agents.is_empty(), "core-tools registers no agents");
    assert!(regs.provider.is_none(), "core-tools registers no provider");

    // `python` is opt-in — the default `new` must NOT register it.
    assert!(
        !names.contains(&"python"),
        "python must be absent unless `.with_python()` is wired; got {names:?}"
    );
}

/// Wiring a `PythonExecutor` via `.with_python()` adds exactly one tool —
/// `python` — on top of the default 15, and nothing else shifts. Locks the
/// opt-in registration branch that the four-arg `new` leaves dormant.
#[tokio::test]
async fn with_python_registers_the_python_tool() {
    let plugin = build_plugin().with_python(Arc::new(StubPython));
    let mut ctx = PluginContext::new(
        plugin.manifest().clone(),
        serde_json::Value::Null,
        Arc::new(NoopCoreApi),
    );

    plugin
        .install(&mut ctx)
        .await
        .expect("core-tools install must succeed with python wired");

    let regs = ctx.into_registrations();
    let names: Vec<&str> = regs.tools.iter().map(|t| t.name()).collect();

    assert_eq!(
        names.len(),
        16,
        "wiring python must register exactly 16 tools, got {names:?}"
    );
    assert!(
        names.contains(&"python"),
        "python tool must be registered when `.with_python()` is set; got {names:?}"
    );
}
