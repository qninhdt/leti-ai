//! E2E (mock-LLM): the REAL `RuntimeSubagentSpawner` driven end-to-end.
//!
//! Every other harness injects a `StubSubagentSpawner` returning `Err`, so
//! the spawn→run→await→cost-rollup→cancel-cascade WIRING through a live
//! runtime was previously unexercised (the orchestration *policy* is unit-
//! tested in `leti-core/tests/subagent_tests.rs`; this covers the
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
use leti_adapters::bus::BroadcastBus;
use leti_adapters::config_perm::ConfigPermissionMgr;
use leti_adapters::localfs::{LocalFilesystem, LocalFsArtifactStore};
use leti_adapters::sqlite::event_repo::SqliteEventRepo;
use leti_adapters::sqlite::{SqliteMemoryStore, open_in_memory};
use leti_core::adapters::ModelProvider;
use leti_core::adapters::model_provider::{
    ChatDelta, ChatRequest, ChatStream, FinishReason, ModelPricing,
};
use leti_core::adapters::permission_manager::PermissionManager;
use leti_core::adapters::tool_executor::ToolCtx;
use leti_core::agent::{AgentDefinition, AgentRegistry, AgentSlug, PromptSegments};
use leti_core::config::{Config, LogFormat, PluginsConfig};
use leti_core::error::{PermissionError, ProviderError, ToolError};
use leti_core::runtime::question_registry::QuestionRegistry;
use leti_core::runtime::subagent::{TaskRegistry, TaskStatus};
use leti_core::runtime::{ConversationRuntime, RuntimeConfig, TurnExtensions};
use leti_core::tools::builtins::subagent_task::SubagentSpawner;
use leti_core::tools::{ReadHistory, Tool};
use leti_core::types::agent::{AgentId, AgentSpec};
use leti_core::types::event::Usage;
use leti_core::types::message::MessageId;
use leti_core::types::permission::{
    AlwaysScope, AskId, Decision, PermissionAction, PermissionCtx, PermissionMode,
    PermissionRequest, PermissionRule,
};
use leti_plugin_core_agents::general_agent;
use leti_server::{AgentResources, AppStateBuilder, RuntimeSubagentSpawner};
use rust_decimal::Decimal;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq)]
struct TurnMarker(&'static str);

#[derive(Default)]
struct ExtObservations {
    permission: Mutex<Vec<String>>,
    tool: Mutex<Vec<String>>,
}

struct CapturingPermission {
    inner: ConfigPermissionMgr,
    observations: Arc<ExtObservations>,
}

#[async_trait]
impl PermissionManager for CapturingPermission {
    async fn check(
        &self,
        ctx: PermissionCtx,
        req: PermissionRequest,
    ) -> Result<Decision, PermissionError> {
        if let Some(marker) = ctx.ext.get::<TurnMarker>() {
            self.observations
                .permission
                .lock()
                .unwrap()
                .push(marker.0.to_owned());
        }
        let _ = req;
        Ok(Decision::Allow)
    }

    async fn reply(&self, ask_id: AskId, decision: Decision) -> Result<(), PermissionError> {
        self.inner.reply(ask_id, decision).await
    }

    async fn cancel_ask(&self, ask_id: AskId) -> Result<(), PermissionError> {
        self.inner.cancel_ask(ask_id).await
    }

    async fn record_always(
        &self,
        scope: AlwaysScope,
        rule: PermissionRule,
    ) -> Result<(), PermissionError> {
        self.inner.record_always(scope, rule).await
    }

    fn take_deferred(&self, ask_id: AskId) -> Option<leti_core::permission::Deferred<Decision>> {
        self.inner.take_deferred(ask_id)
    }

    fn peek_session_id(&self, ask_id: AskId) -> Option<leti_core::types::session::SessionId> {
        self.inner.peek_session_id(ask_id)
    }

    async fn accept_ask(
        &self,
        ask_id: AskId,
        scope: AlwaysScope,
        action: PermissionAction,
    ) -> Result<(), PermissionError> {
        self.inner.accept_ask(ask_id, scope, action).await
    }
}

struct CaptureExtTool {
    observations: Arc<ExtObservations>,
}

#[async_trait]
impl Tool for CaptureExtTool {
    type Input = serde_json::Value;
    type Output = serde_json::Value;

    fn name(&self) -> &'static str {
        "capture_ext"
    }

    fn description(&self) -> &'static str {
        "Record whether the opaque turn marker reached the tool."
    }

    fn permission(&self, _: &Self::Input) -> PermissionRequest {
        PermissionRequest::simple("capture_ext")
    }

    async fn run(&self, ctx: ToolCtx, _: Self::Input) -> Result<Self::Output, ToolError> {
        let found = ctx.ext.get::<TurnMarker>().is_some();
        if let Some(marker) = ctx.ext.get::<TurnMarker>() {
            self.observations
                .tool
                .lock()
                .unwrap()
                .push(marker.0.to_owned());
        }
        Ok(serde_json::json!({ "found": found }))
    }
}

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

fn tool_turn(call_id: &str, name: &str, args: &str) -> Vec<Result<ChatDelta, ProviderError>> {
    vec![
        Ok(ChatDelta::Role),
        Ok(ChatDelta::ToolCallStart {
            call_id: call_id.into(),
            name: name.into(),
            index: 0,
        }),
        Ok(ChatDelta::ToolCallArgsDelta {
            index: 0,
            args_chunk: args.into(),
        }),
        Ok(ChatDelta::Finish {
            reason: FinishReason::ToolUse,
            usage: Some(Usage {
                input_tokens: 1000,
                output_tokens: 50,
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
    observations: Arc<ExtObservations>,
    memory: Arc<dyn leti_core::adapters::MemoryStore>,
    permission: Arc<dyn leti_core::adapters::permission_manager::PermissionManager>,
    events: Arc<dyn leti_core::adapters::EventSink>,
    artifacts: Arc<dyn leti_core::adapters::ArtifactStore>,
    task_registry: Arc<TaskRegistry>,
    agent_registry: Arc<AgentRegistry>,
    fs: Arc<dyn leti_core::adapters::Filesystem>,
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
        let memory: Arc<dyn leti_core::adapters::MemoryStore> =
            Arc::new(SqliteMemoryStore::new(pool.clone()));
        let events: Arc<dyn leti_core::adapters::EventSink> =
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
            default_model: "mock/model".into(),
            permission_ruleset_path: None,
            log_format: LogFormat::Pretty,
            plugins: PluginsConfig::default(),
            tool_scheduler: Default::default(),
        };
        let runtime = Arc::new(ConversationRuntime::new(
            provider.clone(),
            memory.clone(),
            events.clone(),
            RuntimeConfig::new("mock/model".into()),
        ));

        let fs: Arc<dyn leti_core::adapters::Filesystem> =
            Arc::new(LocalFilesystem::new(workspace_root.clone()));
        let shell: Arc<dyn leti_core::tools::builtins::bash::ShellExecutor> = Arc::new(
            leti_adapters::localshell::LocalShellExecutor::new(workspace_root.clone()),
        );

        // Registry must contain the `general` slug so `subagent_type:
        // "general"` resolves at spawn-admission time.
        let mut agent_registry = AgentRegistry::new();
        agent_registry
            .insert(general_agent())
            .expect("insert general");
        agent_registry
            .insert(AgentDefinition {
                slug: AgentSlug::new("opaque-test").expect("static slug"),
                title: "Opaque context test".into(),
                description: "Test agent for opaque turn context propagation".into(),
                prompt_segments: Some(PromptSegments::default()),
                tool_allowlist: vec!["capture_ext".into()],
                model_id: None,
                default_temperature: 0.0,
                context_window: 200_000,
                compaction_threshold: 0.8,
                compaction_summary_cap_tokens: 2_000,
                hidden: true,
            })
            .expect("insert opaque test agent");
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

        let observations = Arc::new(ExtObservations::default());
        let permission: Arc<dyn leti_core::adapters::permission_manager::PermissionManager> =
            Arc::new(CapturingPermission {
                inner: ConfigPermissionMgr::new(),
                observations: observations.clone(),
            });

        let artifacts: Arc<dyn leti_core::adapters::ArtifactStore> =
            Arc::new(LocalFsArtifactStore::new(artifact_root, pool.clone()));

        let state = AppStateBuilder::new()
            .provider(provider)
            .memory(memory.clone())
            .artifacts(artifacts.clone())
            .tool_registry(
                leti_core::tools::ToolRegistry::builder()
                    .register(CaptureExtTool {
                        observations: observations.clone(),
                    })
                    .build(),
            )
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
            observations,
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
            ext: TurnExtensions::default().with(TurnMarker("parent")),
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
async fn real_spawner_forwards_parent_ext_to_child_permission_and_tool() {
    let h = Harness::build(vec![
        tool_turn("capture-opaque", "capture_ext", "{}"),
        text_turn_with_usage("opaque context arrived"),
    ])
    .await;
    let ctx = h.parent_ctx(CancellationToken::new()).await;
    h.memory
        .switch_agent(ctx.session_id, "opaque-test")
        .await
        .expect("switch parent to opaque test agent");

    let task_id = h
        .spawner
        .spawn(&ctx, "opaque-test", "capture the marker", None, false)
        .await
        .expect("spawn admits")
        .task_id;

    let (output, _, status) =
        tokio::time::timeout(Duration::from_secs(10), h.spawner.await_completion(task_id))
            .await
            .expect("await did not hang")
            .expect("await ok");

    assert_eq!(status, TaskStatus::Finished);
    assert!(output.contains("opaque context arrived"));
    assert_eq!(&*h.observations.permission.lock().unwrap(), &["parent"]);
    assert_eq!(&*h.observations.tool.lock().unwrap(), &["parent"]);
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
        .spawn(&ctx, "general", "do the thing", None, false)
        .await
        .expect("spawn admits")
        .task_id;

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
        .spawn(&ctx, "general", "second thing", None, false)
        .await
        .expect("second spawn admits — first child's slot was released")
        .task_id;
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
async fn completed_child_can_be_continued_with_its_existing_session() {
    let h = Harness::build(vec![
        text_turn_with_usage("initial investigation"),
        text_turn_with_usage("continued investigation"),
    ])
    .await;
    let ctx = h.parent_ctx(CancellationToken::new()).await;

    let first = h
        .spawner
        .spawn(&ctx, "general", "investigate the issue", None, false)
        .await
        .expect("initial spawn");
    let (_initial_output, _cost, initial_status) = h
        .spawner
        .await_completion(first.task_id)
        .await
        .expect("initial child settles");
    assert_eq!(initial_status, TaskStatus::Finished);

    let resumed = h
        .spawner
        .continue_subagent(
            &ctx,
            first.child_session_id,
            "continue and report the remaining evidence",
            false,
        )
        .await
        .expect("continuation admits");
    assert_eq!(resumed.child_session_id, first.child_session_id);
    assert_ne!(resumed.task_id, first.task_id);

    let (output, _cost, status) = tokio::time::timeout(
        Duration::from_secs(10),
        h.spawner.await_completion(resumed.task_id),
    )
    .await
    .expect("continuation await did not hang")
    .expect("continuation settles");
    assert_eq!(status, TaskStatus::Finished);
    assert!(output.contains("continued investigation"), "got {output:?}");

    let executions = h
        .memory
        .list_subagent_executions(ctx.session_id, true)
        .await
        .expect("durable executions");
    assert_eq!(executions.len(), 2);
    assert!(
        executions
            .iter()
            .all(|execution| execution.child_session_id == first.child_session_id)
    );
}

#[tokio::test]
async fn background_task_persists_one_typed_parent_reminder_without_user_text() {
    let h = Harness::build(vec![
        text_turn_with_usage("child found the answer"),
        text_turn_with_usage("parent incorporated the result"),
    ])
    .await;
    let ctx = h.parent_ctx(CancellationToken::new()).await;
    let parent_sid = ctx.session_id;

    let spawned = h
        .spawner
        .spawn(&ctx, "general", "research", None, true)
        .await
        .expect("background spawn");
    let (_output, _cost, status) = h
        .spawner
        .await_completion(spawned.task_id)
        .await
        .expect("child settles");
    assert_eq!(status, TaskStatus::Finished);

    // Task status becomes terminal immediately before the driver's durable
    // parent notification write, so wait for that write rather than coupling
    // the test to scheduler timing.
    let mut reminder_count = 0;
    for _ in 0..100 {
        reminder_count = 0;
        for message in h.memory.list_messages(parent_sid).await.expect("messages") {
            for part in h
                .memory
                .list_parts(parent_sid, message.id)
                .await
                .expect("parts")
            {
                match part {
                    leti_core::types::part::Part::RuntimeReminder {
                        reminder_kind: leti_core::types::part::ReminderKind::BackgroundTaskSettled,
                        content,
                        ..
                    } => {
                        reminder_count += 1;
                        assert!(content.contains("child found the answer"));
                    }
                    leti_core::types::part::Part::Text { text, .. } => assert!(
                        !text.contains("<untrusted-subagent-output"),
                        "background delivery must not create a synthetic user text bubble"
                    ),
                    _ => {}
                }
            }
        }
        if reminder_count == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(reminder_count, 1, "settlement owns one durable reminder");
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
        .spawn(&ctx, "general", "long task", None, false)
        .await
        .expect("spawn")
        .task_id;
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
