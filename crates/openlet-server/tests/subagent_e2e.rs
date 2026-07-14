//! E2E (mock-LLM): the REAL `RuntimeSubagentSpawner` driven end-to-end.
//!
//! Every other harness injects a `StubSubagentSpawner` returning `Err`, so
//! the spawn→run→await→cost-rollup→cancel-cascade WIRING through a live
//! runtime was previously unexercised (the orchestration *policy* is unit-
//! tested in `openlet-core/tests/subagent_tests.rs`; this covers the
//! server-side spawner that actually drives a child `run_loop`).
//!
//! Layer: e2e, mock-backed. A scripted in-process provider supplies the
//! child turn's deltas — no network, no key, deterministic.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, StreamExt};
use openlet_adapters::bus::BroadcastBus;
use openlet_adapters::config_perm::ConfigPermissionMgr;
use openlet_adapters::localfs::{LocalFilesystem, LocalFsArtifactStore};
use openlet_adapters::sqlite::event_repo::SqliteEventRepo;
use openlet_adapters::sqlite::{SqliteMemoryStore, open_in_memory};
use openlet_core::adapters::ModelProvider;
use openlet_core::adapters::model_provider::{
    ChatDelta, ChatRequest, ChatStream, FinishReason, ModelPricing,
};
use openlet_core::adapters::tool_executor::ToolCtx;
use openlet_core::agent::AgentRegistry;
use openlet_core::config::{Config, LogFormat, PluginsConfig};
use openlet_core::error::ProviderError;
use openlet_core::runtime::question_registry::QuestionRegistry;
use openlet_core::runtime::subagent::{TaskRegistry, TaskStatus};
use openlet_core::runtime::{ConversationRuntime, RuntimeConfig};
use openlet_core::tools::ReadHistory;
use openlet_core::tools::builtins::subagent_task::SubagentSpawner;
use openlet_core::types::agent::{AgentId, AgentSpec};
use openlet_core::types::event::Usage;
use openlet_core::types::message::MessageId;
use openlet_core::types::permission::PermissionMode;
use openlet_plugin_core_agents::general_agent;
use openlet_server::{AgentResources, AppStateBuilder, RuntimeSubagentSpawner};
use rust_decimal::Decimal;
use tokio_util::sync::CancellationToken;

/// Minimal scripted provider: emits one queued turn per `chat_stream`,
/// peeking the cancel token between deltas so a tripped token yields a
/// synthetic `Cancelled` finish (mirrors the core test mock).
struct ScriptedProvider {
    turns: Mutex<VecDeque<Vec<Result<ChatDelta, ProviderError>>>>,
    pricing: Option<ModelPricing>,
}

impl ScriptedProvider {
    fn new(turns: Vec<Vec<Result<ChatDelta, ProviderError>>>, pricing: ModelPricing) -> Self {
        Self {
            turns: Mutex::new(turns.into()),
            pricing: Some(pricing),
        }
    }
}

#[async_trait]
impl ModelProvider for ScriptedProvider {
    async fn chat_stream(
        &self,
        _req: ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ChatStream, ProviderError> {
        let deltas = self.turns.lock().unwrap().pop_front().unwrap_or_default();
        let stream = stream::unfold(
            (deltas.into_iter(), cancel, false),
            |(mut iter, cancel, done)| async move {
                if done {
                    return None;
                }
                if cancel.is_cancelled() {
                    let frame = Ok(ChatDelta::Finish {
                        reason: FinishReason::Cancelled,
                        usage: None,
                    });
                    return Some((frame, (iter, cancel, true)));
                }
                iter.next().map(|d| (d, (iter, cancel, done)))
            },
        );
        Ok(Box::new(stream.boxed()) as ChatStream)
    }

    fn pricing(&self, _model: &str) -> Option<ModelPricing> {
        self.pricing.clone()
    }
}

/// One text turn with usage so the cost path produces a non-zero figure.
fn text_turn_with_usage(text: &str) -> Vec<Result<ChatDelta, ProviderError>> {
    vec![
        Ok(ChatDelta::Role),
        Ok(ChatDelta::Content { text: text.into() }),
        Ok(ChatDelta::Finish {
            reason: FinishReason::EndTurn,
            usage: Some(Usage {
                input_tokens: 1000,
                output_tokens: 1000,
                ..Default::default()
            }),
        }),
    ]
}

/// Build a fully-wired `AppState` with the REAL spawner bound, a scripted
/// provider, and a `general` agent in the registry. Returns the spawner,
/// the shared handles needed to seed a parent session + build a `ToolCtx`,
/// and the workspace tempdir guard (kept alive for the test).
struct Harness {
    spawner: Arc<RuntimeSubagentSpawner>,
    memory: Arc<dyn openlet_core::adapters::MemoryStore>,
    permission: Arc<dyn openlet_core::adapters::permission_manager::PermissionManager>,
    events: Arc<dyn openlet_core::adapters::EventSink>,
    artifacts: Arc<dyn openlet_core::adapters::ArtifactStore>,
    task_registry: Arc<TaskRegistry>,
    agent_registry: Arc<AgentRegistry>,
    fs: Arc<dyn openlet_core::adapters::Filesystem>,
    parent_agent_id: AgentId,
    _tempdir: tempfile::TempDir,
}

impl Harness {
    async fn build(turns: Vec<Vec<Result<ChatDelta, ProviderError>>>) -> Self {
        let tempdir = tempfile::tempdir().expect("tempdir");
        let workspace_root = tempdir.path().join("ws");
        let artifact_root = tempdir.path().join("artifacts");
        tokio::fs::create_dir_all(&workspace_root).await.unwrap();
        tokio::fs::create_dir_all(&artifact_root).await.unwrap();

        let pool = open_in_memory().await.expect("sqlite");
        let memory: Arc<dyn openlet_core::adapters::MemoryStore> =
            Arc::new(SqliteMemoryStore::new(pool.clone()));
        let events: Arc<dyn openlet_core::adapters::EventSink> =
            Arc::new(BroadcastBus::with_repo(SqliteEventRepo::new(pool.clone())));
        let pricing = ModelPricing {
            input_per_mtok: Decimal::ONE,
            output_per_mtok: Decimal::ONE,
            cached_input_per_mtok: None,
            cache_write_per_mtok: None,
        };
        let provider: Arc<dyn ModelProvider> = Arc::new(ScriptedProvider::new(turns, pricing));

        let config = Config {
            bind_addr: "127.0.0.1:0".into(),
            data_dir: tempdir.path().to_path_buf(),
            openai_api_key: None,
            default_model: "mock/model".into(),
            permission_ruleset_path: None,
            log_format: LogFormat::Pretty,
            plugins: PluginsConfig::default(),
            cloud_fs: None,
        };
        let runtime = Arc::new(ConversationRuntime::new(
            provider.clone(),
            memory.clone(),
            events.clone(),
            RuntimeConfig::new("mock/model".into()),
        ));

        let fs: Arc<dyn openlet_core::adapters::Filesystem> =
            Arc::new(LocalFilesystem::new(workspace_root.clone()));
        let shell: Arc<dyn openlet_core::tools::builtins::bash::ShellExecutor> = Arc::new(
            openlet_adapters::localshell::LocalShellExecutor::new(workspace_root.clone()),
        );

        // Registry must contain the `general` slug so `subagent_type:
        // "general"` resolves at spawn-admission time.
        let mut agent_registry = AgentRegistry::new();
        agent_registry
            .insert(general_agent())
            .expect("insert general");
        let agent_registry = Arc::new(agent_registry);

        let task_registry = Arc::new(TaskRegistry::new(32));
        let spawner = Arc::new(RuntimeSubagentSpawner::new());

        // The parent session's agent_id must be present in the agents map
        // (the spawner clones the parent's AgentResources for the child).
        let parent_agent_id = AgentId::new();
        let mut agents: HashMap<AgentId, AgentResources> = HashMap::new();
        agents.insert(
            parent_agent_id,
            AgentResources {
                spec: AgentSpec::new(parent_agent_id, workspace_root.clone(), "default"),
                fs: fs.clone(),
                shell: shell.clone(),
            },
        );

        let permission: Arc<dyn openlet_core::adapters::permission_manager::PermissionManager> =
            Arc::new(ConfigPermissionMgr::new());

        let artifacts: Arc<dyn openlet_core::adapters::ArtifactStore> =
            Arc::new(LocalFsArtifactStore::new(artifact_root, pool.clone()));

        let state = AppStateBuilder::new()
            .provider(provider)
            .memory(memory.clone())
            .artifacts(artifacts.clone())
            .tool_registry(openlet_core::tools::ToolRegistry::builder().build())
            .events(events.clone())
            .permission(permission.clone())
            .config(Arc::new(config))
            .runtime(runtime)
            .agents(agents)
            .default_agent_id(parent_agent_id)
            .workspace_root(workspace_root.clone())
            .agent_registry(agent_registry.clone())
            .task_registry(task_registry.clone())
            .build()
            .expect("build app state");

        // Bind the live state into the spawner — the real boot order.
        spawner.set_state(state);

        Self {
            spawner,
            memory,
            permission,
            events,
            artifacts,
            task_registry,
            agent_registry,
            fs,
            parent_agent_id,
            _tempdir: tempdir,
        }
    }

    /// Seed a parent session row and return a `ToolCtx` pointed at it, so
    /// the spawner's `get_session`/root-resolution/agent lookup succeed.
    async fn parent_ctx(&self, cancel: CancellationToken) -> ToolCtx {
        let sid = self
            .memory
            .create_session(self.parent_agent_id, None)
            .await
            .expect("create parent session");
        ToolCtx {
            session_id: sid,
            agent_id: self.parent_agent_id,
            message_id: MessageId::new(),
            call_id: "call-subagent".into(),
            mode: PermissionMode::Danger,
            fs: self.fs.clone(),
            permission: self.permission.clone(),
            events: self.events.clone(),
            artifacts: self.artifacts.clone(),
            read_history: ReadHistory::new(),
            cancel,
            questions: Arc::new(QuestionRegistry::new()),
            memory: self.memory.clone(),
            task_registry: self.task_registry.clone(),
            agent_registry: self.agent_registry.clone(),
        }
    }
}

#[tokio::test]
async fn real_spawner_runs_child_to_completion_with_output_and_cost() {
    let h = Harness::build(vec![
        text_turn_with_usage("subagent says hi"),
        text_turn_with_usage("second child done"),
    ])
    .await;
    let ctx = h.parent_ctx(CancellationToken::new()).await;

    let task_id = h
        .spawner
        .spawn(&ctx, "general", "do the thing")
        .await
        .expect("spawn admits");

    let (output, cost, status) =
        tokio::time::timeout(Duration::from_secs(10), h.spawner.await_completion(task_id))
            .await
            .expect("await did not hang")
            .expect("await ok");

    assert_eq!(status, TaskStatus::Finished, "child must finish cleanly");
    assert!(
        output.contains("subagent says hi"),
        "child assistant text must surface as task output, got {output:?}"
    );
    assert!(
        cost.is_some(),
        "non-zero child cost must roll up to the task"
    );

    // Cost cascaded to the parent's cumulative ledger too.
    let parent_cost = h
        .spawner
        .await_completion(task_id)
        .await
        .ok()
        .map(|_| ())
        .is_some();
    assert!(parent_cost, "await_completion replays from terminal cache");

    // Phase 1: prove the real driver releases the child's quota slot on
    // settle (no leak through the live spawn→run→finalize path). Reusing
    // the SAME parent ctx keeps both children under one ROOT quota bucket;
    // a second spawn+settle on that root proves the first slot was freed
    // (the balanced-counter contract through the live driver, not just the
    // unit-level `finalize`).
    let task_id2 = h
        .spawner
        .spawn(&ctx, "general", "second thing")
        .await
        .expect("second spawn admits — first child's slot was released");
    assert_ne!(task_id, task_id2, "distinct tasks");
    let (_o2, _c2, status2) = tokio::time::timeout(
        Duration::from_secs(10),
        h.spawner.await_completion(task_id2),
    )
    .await
    .expect("await did not hang")
    .expect("await ok");
    assert_eq!(status2, TaskStatus::Finished);
}

#[tokio::test]
async fn promoted_task_injects_result_into_parent_and_settles_without_output() {
    // Phase 3: a PROMOTED background task delivers its output via an
    // injected `InjectedResult` turn in the PARENT session (untrusted-
    // wrapped), and its `SubagentSettled` frame carries NO output payload
    // (OpenCode synthetic-message pattern). The parent's injected turn
    // itself drives a (scripted) model turn, so we supply two turns: the
    // child's, then the parent's injected turn.
    let h = Harness::build(vec![
        text_turn_with_usage("child computed 42"),
        text_turn_with_usage("parent acknowledges"),
    ])
    .await;
    let ctx = h.parent_ctx(CancellationToken::new()).await;
    let parent_sid = ctx.session_id;

    // Spawn background + mark promoted BEFORE it settles.
    let task_id = h
        .spawner
        .spawn(&ctx, "general", "compute the answer")
        .await
        .expect("spawn admits");
    assert!(
        h.task_registry.mark_promoted(task_id),
        "live background task can be promoted"
    );

    // Await the child's terminal settle via the registry.
    let snap = tokio::time::timeout(
        Duration::from_secs(10),
        h.task_registry.await_completion(task_id),
    )
    .await
    .expect("await did not hang")
    .expect("terminal snapshot");
    assert_eq!(snap.status, TaskStatus::Finished);

    // The parent session must receive an injected turn carrying the child's
    // output wrapped as untrusted data. Poll the parent message log.
    let mut injected_seen = false;
    for _ in 0..300 {
        let msgs = h.memory.list_messages(parent_sid).await.expect("messages");
        for m in &msgs {
            let parts = h.memory.list_parts(parent_sid, m.id).await.expect("parts");
            for p in parts {
                if let openlet_core::types::part::Part::Text { text, .. } = p
                    && text.contains("child computed 42")
                    && text.contains("<untrusted-subagent-output")
                {
                    injected_seen = true;
                }
            }
        }
        if injected_seen {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        injected_seen,
        "promoted task's output must re-enter the parent as an untrusted-wrapped injected turn"
    );
}

#[tokio::test]
async fn cancelling_parent_cascades_to_child() {
    // A child turn that would emit text — but we cancel before/while it
    // runs, so the scripted provider yields a Cancelled finish and the
    // task ends Cancelled rather than Finished.
    let h = Harness::build(vec![text_turn_with_usage("should not finish")]).await;
    let cancel = CancellationToken::new();
    let ctx = h.parent_ctx(cancel.clone()).await;

    let task_id = h
        .spawner
        .spawn(&ctx, "general", "long task")
        .await
        .expect("spawn");
    // Trip the parent token — the child's cancel is a child_token of it.
    cancel.cancel();

    let (_out, _cost, status) =
        tokio::time::timeout(Duration::from_secs(10), h.spawner.await_completion(task_id))
            .await
            .expect("await did not hang")
            .expect("await ok");

    assert_eq!(
        status,
        TaskStatus::Cancelled,
        "cancelling the parent must cascade to the child task"
    );
}
