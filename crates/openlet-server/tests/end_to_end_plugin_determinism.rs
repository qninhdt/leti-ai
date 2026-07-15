//! End-to-end regression gate: plugin registration determinism.
//!
//! `install_all` over the canonical plugin set must produce the same
//! ordered tool/agent list on every boot. Non-determinism here would
//! mean a tool collision could win different rounds in different
//! processes, or hooks could fire in different priority-bucket orders
//! across replicas.
//!
//! Complement to `integration_smoke::canonical_plugin_set_drains_tools_and_agents`
//! (which only asserts presence) — we assert *order stability* across
//! 5 boots to catch hash-randomization or `HashMap` iteration drift.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use openlet_core::adapters::tool_executor::ToolCtx;
use openlet_core::error::ToolError;
use openlet_core::tools::builtins::bash::{BashOutput, ShellExecutor};
use openlet_core::types::session::{SessionId, SessionMeta};
use openlet_plugin_api::context::CoreApi;
use openlet_plugin_registry::{all_plugins, install_all};
use rust_decimal::Decimal;

#[tokio::test]
async fn install_all_tool_order_is_stable_across_boots() {
    let mut runs: Vec<Vec<String>> = Vec::new();
    for _ in 0..5 {
        let core_api: Arc<dyn CoreApi> = Arc::new(NoopCoreApi);
        let configs = HashMap::new();
        let plugins = all_plugins(
            stub_shell(),
            None,
            stub_memory(),
            stub_task_registry(),
            stub_spawner(),
        );
        let installed = install_all(plugins, &configs, core_api).await.unwrap();
        let names: Vec<String> = installed
            .tools
            .iter()
            .map(|t| t.name().to_string())
            .collect();
        runs.push(names);
    }

    let first = &runs[0];
    for (i, run) in runs.iter().enumerate().skip(1) {
        assert_eq!(
            run, first,
            "tool registration order drifted at boot {i}: \
             first={first:?}, this run={run:?}"
        );
    }
}

#[tokio::test]
async fn install_all_agent_order_is_stable_across_boots() {
    let mut runs: Vec<Vec<String>> = Vec::new();
    for _ in 0..5 {
        let core_api: Arc<dyn CoreApi> = Arc::new(NoopCoreApi);
        let configs = HashMap::new();
        let plugins = all_plugins(
            stub_shell(),
            None,
            stub_memory(),
            stub_task_registry(),
            stub_spawner(),
        );
        let installed = install_all(plugins, &configs, core_api).await.unwrap();
        let slugs: Vec<String> = installed
            .agents
            .iter()
            .map(|a| a.slug.as_str().to_owned())
            .collect();
        runs.push(slugs);
    }

    let first = &runs[0];
    for (i, run) in runs.iter().enumerate().skip(1) {
        assert_eq!(
            run, first,
            "agent registration order drifted at boot {i}: \
             first={first:?}, this run={run:?}"
        );
    }
}

#[tokio::test]
async fn install_all_hook_chain_lengths_stable_across_boots() {
    let mut before_turn = Vec::new();
    let mut on_cost_tick = Vec::new();
    let mut on_chat_params = Vec::new();
    let mut on_chat_messages = Vec::new();
    let mut after_tool_call = Vec::new();
    for _ in 0..5 {
        let core_api: Arc<dyn CoreApi> = Arc::new(NoopCoreApi);
        let configs = HashMap::new();
        let plugins = all_plugins(
            stub_shell(),
            None,
            stub_memory(),
            stub_task_registry(),
            stub_spawner(),
        );
        let installed = install_all(plugins, &configs, core_api).await.unwrap();
        before_turn.push(installed.chains.before_turn.len());
        on_cost_tick.push(installed.chains.on_cost_tick.len());
        on_chat_params.push(installed.chains.on_chat_params.len());
        on_chat_messages.push(installed.chains.on_chat_messages.len());
        after_tool_call.push(installed.chains.after_tool_call.len());
    }
    let stable = |xs: &[usize], name: &str| {
        let first = xs[0];
        for (i, &n) in xs.iter().enumerate().skip(1) {
            assert_eq!(n, first, "{name} chain length drifted at boot {i}: {xs:?}");
        }
    };
    stable(&before_turn, "before_turn");
    stable(&on_cost_tick, "on_cost_tick");
    stable(&on_chat_params, "on_chat_params");
    stable(&on_chat_messages, "on_chat_messages");
    stable(&after_tool_call, "after_tool_call");
}

#[tokio::test]
async fn canonical_built_in_tools_appear_with_no_collision() {
    // Locks: every built-in tool name appears exactly once in the
    // installed list. A duplicate (or missing) entry would mean a
    // plugin double-registered or `register_tool` silently dropped.
    let core_api: Arc<dyn CoreApi> = Arc::new(NoopCoreApi);
    let configs = HashMap::new();
    let plugins = all_plugins(
        stub_shell(),
        None,
        stub_memory(),
        stub_task_registry(),
        stub_spawner(),
    );
    let installed = install_all(plugins, &configs, core_api).await.unwrap();
    let mut names: Vec<&'static str> = installed.tools.iter().map(|t| t.name()).collect();
    names.sort();
    let mut deduped = names.clone();
    deduped.dedup();
    assert_eq!(
        names, deduped,
        "tool name collision detected — duplicate(s) in {names:?}"
    );
    for built_in in [
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
            names.contains(&built_in),
            "missing built-in `{built_in}` from {names:?}"
        );
    }
}

// --- stubs (kept narrow; complement integration_smoke's helpers) ---

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
    async fn emit_event(
        &self,
        _: openlet_core::types::event::AgentEvent,
        _: openlet_core::adapters::event_sink::Persistence,
    ) {
    }
    fn read_config(&self, _: &str) -> Result<serde_json::Value, String> {
        Ok(serde_json::Value::Null)
    }
    async fn cancel_session(&self, _: SessionId, _: String) {}
    async fn emit_notification(
        &self,
        _: Option<SessionId>,
        _: openlet_core::hooks::io::NotificationLevel,
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

fn stub_memory() -> Arc<dyn openlet_core::adapters::memory_store::MemoryStore> {
    use openlet_core::adapters::memory_store::MemoryStore;
    use openlet_core::error::MemoryError;

    struct NoopMemory;

    #[async_trait]
    impl MemoryStore for NoopMemory {
        async fn create_session(
            &self,
            _: openlet_core::types::agent::AgentId,
            _: Option<SessionId>,
        ) -> Result<SessionId, MemoryError> {
            Ok(SessionId::new())
        }
        async fn get_session(&self, _: SessionId) -> Result<Option<SessionMeta>, MemoryError> {
            Ok(None)
        }
        async fn list_sessions(
            &self,
            _: openlet_core::types::session::SessionFilter,
        ) -> Result<Vec<SessionMeta>, MemoryError> {
            Ok(Vec::new())
        }
        async fn update_status(
            &self,
            _: SessionId,
            _: openlet_core::types::session::SessionStatus,
            _: &str,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn update_permission_mode(
            &self,
            _: SessionId,
            _: openlet_core::types::permission::PermissionMode,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn switch_agent(&self, _: SessionId, _: &str) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn update_session_extensions(
            &self,
            _: SessionId,
            _: serde_json::Value,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn delete_session(&self, _: SessionId) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn append_message(
            &self,
            _: SessionId,
            msg: openlet_core::types::message::Message,
        ) -> Result<openlet_core::types::message::MessageId, MemoryError> {
            Ok(msg.id)
        }
        async fn append_part(
            &self,
            _: openlet_core::types::message::MessageId,
            part: openlet_core::types::part::Part,
        ) -> Result<openlet_core::types::part::PartId, MemoryError> {
            Ok(part.id())
        }
        async fn upsert_part(
            &self,
            _: openlet_core::types::message::MessageId,
            _: openlet_core::types::part::PartId,
            _: openlet_core::types::part::Part,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn list_messages(
            &self,
            _: SessionId,
        ) -> Result<Vec<openlet_core::types::message::Message>, MemoryError> {
            Ok(Vec::new())
        }
        async fn list_parts(
            &self,
            _: SessionId,
            _: openlet_core::types::message::MessageId,
        ) -> Result<Vec<openlet_core::types::part::Part>, MemoryError> {
            Ok(Vec::new())
        }
        async fn record_read(
            &self,
            _: SessionId,
            _: std::path::PathBuf,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    Arc::new(NoopMemory)
}

fn stub_task_registry() -> Arc<openlet_core::runtime::subagent::TaskRegistry> {
    Arc::new(openlet_core::runtime::subagent::TaskRegistry::new(32))
}

fn stub_spawner() -> Arc<dyn openlet_core::tools::builtins::subagent_task::SubagentSpawner> {
    use openlet_core::runtime::subagent::{SpawnError, TaskId, TaskStatus};
    use openlet_core::tools::builtins::subagent_task::SpawnedSubagent;

    struct StubSpawner;

    #[async_trait]
    impl openlet_core::tools::builtins::subagent_task::SubagentSpawner for StubSpawner {
        async fn spawn(
            &self,
            _ctx: &ToolCtx,
            _subagent_type: &str,
            _objective: &str,
            _scope: Option<&str>,
            _background: bool,
        ) -> Result<SpawnedSubagent, SpawnError> {
            Err(SpawnError::Internal("stub".into()))
        }
        async fn await_completion(
            &self,
            _task_id: TaskId,
        ) -> Result<(String, Option<String>, TaskStatus), SpawnError> {
            Err(SpawnError::Internal("stub".into()))
        }
    }

    Arc::new(StubSpawner)
}
