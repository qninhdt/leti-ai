//! Tests for the `enter_plan_mode` / `exit_plan_mode` tools and the
//! per-dispatch allowlist enforcement.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use openlet_adapters::localfs::LocalFilesystem;
use openlet_core::adapters::artifact_store::{ArtifactRef, ArtifactStore};
use openlet_core::adapters::event_sink::{DeliveredEvent, EventSink, Persistence};
use openlet_core::adapters::memory_store::MemoryStore;
use openlet_core::adapters::permission_manager::PermissionManager;
use openlet_core::adapters::tool_executor::ToolCtx;
use openlet_core::error::{ArtifactError, EventError, MemoryError, PermissionError};
use openlet_core::tools::ReadHistory;
use openlet_core::tools::Tool;
use openlet_core::tools::builtins::plan_mode::{
    EnterPlanModeInput, ExitPlanModeInput, PLAN_AGENT_SLUG,
};
use openlet_core::tools::builtins::{EnterPlanModeTool, ExitPlanModeTool};
use openlet_core::types::agent::AgentId;
use openlet_core::types::event::{AgentEvent, EventFilter};
use openlet_core::types::message::{Message, MessageId};
use openlet_core::types::part::{Part, PartId};
use openlet_core::types::permission::{
    AlwaysScope, AskId, Decision, PermissionAction, PermissionCtx, PermissionMode,
    PermissionRequest, PermissionRule,
};
use openlet_core::types::session::{SessionFilter, SessionId, SessionMeta, SessionStatus};
use tempfile::TempDir;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct AllowAll;

#[async_trait]
impl PermissionManager for AllowAll {
    async fn check(
        &self,
        _: PermissionCtx,
        _: PermissionRequest,
    ) -> Result<Decision, PermissionError> {
        Ok(Decision::Allow)
    }
    async fn reply(&self, _: AskId, _: Decision) -> Result<(), PermissionError> {
        Ok(())
    }
    async fn cancel_ask(&self, _: AskId) -> Result<(), PermissionError> {
        Ok(())
    }
    async fn record_always(
        &self,
        _: AlwaysScope,
        _: PermissionRule,
    ) -> Result<(), PermissionError> {
        Ok(())
    }
    fn take_deferred(&self, _: AskId) -> Option<openlet_core::permission::Deferred<Decision>> {
        None
    }
    fn peek_session_id(&self, _: AskId) -> Option<SessionId> {
        None
    }
    async fn accept_ask(
        &self,
        _: AskId,
        _: AlwaysScope,
        _: PermissionAction,
    ) -> Result<(), PermissionError> {
        Ok(())
    }
}

#[derive(Default)]
struct DiscardArtifacts;

#[async_trait]
impl ArtifactStore for DiscardArtifacts {
    async fn put(
        &self,
        session: SessionId,
        key: &str,
        _: Bytes,
    ) -> Result<ArtifactRef, ArtifactError> {
        Ok(ArtifactRef {
            session_id: session,
            key: key.to_string(),
            size: 0,
            mime: None,
        })
    }
    async fn get(&self, _: &ArtifactRef) -> Result<Bytes, ArtifactError> {
        Err(ArtifactError::NotFound("test".into()))
    }
    async fn list(&self, _: SessionId) -> Result<Vec<ArtifactRef>, ArtifactError> {
        Ok(vec![])
    }
}

/// Recording event sink — stores every published event in a vector
/// the tests can introspect.
#[derive(Default)]
struct RecordingBus {
    events: Mutex<Vec<AgentEvent>>,
}

#[async_trait]
impl EventSink for RecordingBus {
    async fn publish(&self, ev: AgentEvent, _: Persistence) -> Result<(), EventError> {
        self.events.lock().unwrap().push(ev);
        Ok(())
    }
    fn subscribe(&self, _: EventFilter) -> broadcast::Receiver<DeliveredEvent> {
        let (_, rx) = broadcast::channel(1);
        rx
    }
}

/// Minimal in-memory MemoryStore — only the methods plan_mode tools
/// actually call need real behaviour. The rest are stubs.
struct InMemoryStore {
    sessions: Mutex<std::collections::HashMap<SessionId, SessionMeta>>,
    messages: Mutex<Vec<(SessionId, Message)>>,
    parts: Mutex<std::collections::HashMap<MessageId, Vec<Part>>>,
}

impl InMemoryStore {
    fn new() -> Self {
        Self {
            sessions: Mutex::new(std::collections::HashMap::new()),
            messages: Mutex::new(Vec::new()),
            parts: Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn seed_session(&self, meta: SessionMeta) {
        self.sessions.lock().unwrap().insert(meta.id, meta);
    }

    fn get_meta(&self, sid: SessionId) -> SessionMeta {
        self.sessions.lock().unwrap().get(&sid).cloned().unwrap()
    }

    fn parts_for(&self, mid: MessageId) -> Vec<Part> {
        self.parts
            .lock()
            .unwrap()
            .get(&mid)
            .cloned()
            .unwrap_or_default()
    }
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MemoryStore for InMemoryStore {
    async fn create_session(
        &self,
        _: AgentId,
        _: Option<SessionId>,
    ) -> Result<SessionId, MemoryError> {
        Ok(SessionId::new())
    }
    async fn get_session(&self, sid: SessionId) -> Result<Option<SessionMeta>, MemoryError> {
        Ok(self.sessions.lock().unwrap().get(&sid).cloned())
    }
    async fn list_sessions(&self, _: SessionFilter) -> Result<Vec<SessionMeta>, MemoryError> {
        Ok(self.sessions.lock().unwrap().values().cloned().collect())
    }
    async fn update_status(
        &self,
        _: SessionId,
        _: SessionStatus,
        _: &str,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn update_permission_mode(
        &self,
        _: SessionId,
        _: PermissionMode,
    ) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn switch_agent(&self, sid: SessionId, slug: &str) -> Result<(), MemoryError> {
        let mut g = self.sessions.lock().unwrap();
        let meta = g.get_mut(&sid).ok_or(MemoryError::SessionNotFound)?;
        meta.previous_agent_slug = meta.current_agent_slug.clone();
        meta.current_agent_slug = Some(slug.to_string());
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
    async fn append_message(&self, sid: SessionId, msg: Message) -> Result<MessageId, MemoryError> {
        let id = msg.id;
        self.messages.lock().unwrap().push((sid, msg));
        Ok(id)
    }
    async fn append_part(&self, mid: MessageId, part: Part) -> Result<PartId, MemoryError> {
        let id = part.id();
        self.parts
            .lock()
            .unwrap()
            .entry(mid)
            .or_default()
            .push(part);
        Ok(id)
    }
    async fn upsert_part(&self, _: MessageId, _: PartId, _: Part) -> Result<(), MemoryError> {
        Ok(())
    }
    async fn list_messages(&self, sid: SessionId) -> Result<Vec<Message>, MemoryError> {
        Ok(self
            .messages
            .lock()
            .unwrap()
            .iter()
            .filter(|(s, _)| *s == sid)
            .map(|(_, m)| m.clone())
            .collect())
    }
    async fn list_parts(&self, _: SessionId, mid: MessageId) -> Result<Vec<Part>, MemoryError> {
        Ok(self.parts_for(mid))
    }
    async fn record_read(&self, _: SessionId, _: PathBuf) -> Result<(), MemoryError> {
        Ok(())
    }
}

fn ctx_with_bus(workspace: &std::path::Path, sid: SessionId, bus: Arc<RecordingBus>) -> ToolCtx {
    use openlet_core::runtime::QuestionRegistry;
    let memory: Arc<dyn MemoryStore> = Arc::new(InMemoryStore::default());
    ToolCtx {
        session_id: sid,
        agent_id: AgentId::new(),
        message_id: MessageId::new(),
        call_id: "call-1".into(),
        fs: Arc::new(LocalFilesystem::new(workspace.to_path_buf())),
        mode: PermissionMode::Danger,
        permission: Arc::new(AllowAll),
        events: bus,
        artifacts: Arc::new(DiscardArtifacts),
        read_history: ReadHistory::new(),
        cancel: CancellationToken::new(),
        questions: Arc::new(QuestionRegistry::new()),
        memory,
        task_registry: Arc::new(openlet_core::runtime::subagent::TaskRegistry::new(32)),
        agent_registry: Arc::new(openlet_core::agent::AgentRegistry::new()),
    }
}

fn seed_meta(sid: SessionId, current: Option<&str>, previous: Option<&str>) -> SessionMeta {
    SessionMeta {
        id: sid,
        agent_id: AgentId::new(),
        status: SessionStatus::Running,
        permission_mode: PermissionMode::Danger,
        parent_session_id: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        deleted_at: None,
        version: "0.1.0".into(),
        extensions: serde_json::Value::Null,
        capabilities: openlet_core::types::session::SessionCapabilities::default(),
        current_agent_slug: current.map(str::to_string),
        previous_agent_slug: previous.map(str::to_string),
        depth: 0,
        model: None,
    }
}

#[tokio::test]
async fn enter_plan_mode_emits_event_and_switches_agent() {
    let dir = TempDir::new().unwrap();
    let sid = SessionId::new();
    let store = Arc::new(InMemoryStore::new());
    store.seed_session(seed_meta(sid, Some("general"), None));
    let bus = Arc::new(RecordingBus::default());

    let tool = EnterPlanModeTool::new(store.clone());
    let ctx = ctx_with_bus(dir.path(), sid, bus.clone());
    let out = tool.run(ctx, EnterPlanModeInput::default()).await.unwrap();
    assert_eq!(out.agent, PLAN_AGENT_SLUG);

    let meta = store.get_meta(sid);
    assert_eq!(meta.current_agent_slug.as_deref(), Some("plan"));
    assert_eq!(meta.previous_agent_slug.as_deref(), Some("general"));

    let events = bus.events.lock().unwrap();
    assert!(matches!(
        events.first(),
        Some(AgentEvent::PlanModeEntered { .. })
    ));
}

#[tokio::test]
async fn exit_plan_mode_with_plan_attaches_plan_part_and_restores_agent() {
    let dir = TempDir::new().unwrap();
    let sid = SessionId::new();
    let store = Arc::new(InMemoryStore::new());
    store.seed_session(seed_meta(sid, Some("plan"), Some("general")));
    let bus = Arc::new(RecordingBus::default());

    let tool = ExitPlanModeTool::new(store.clone());
    let ctx = ctx_with_bus(dir.path(), sid, bus.clone());
    let out = tool
        .run(
            ctx,
            ExitPlanModeInput {
                plan: "1. read X\n2. modify Y".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(out.restored_agent, "general");
    assert!(out.was_in_plan_mode);

    let meta = store.get_meta(sid);
    assert_eq!(meta.current_agent_slug.as_deref(), Some("general"));

    // A Tool-role message holding Part::Plan should be appended.
    let messages = store.messages.lock().unwrap();
    assert_eq!(messages.len(), 1);
    let mid = messages[0].1.id;
    drop(messages);
    let parts = store.parts_for(mid);
    assert!(
        parts
            .iter()
            .any(|p| matches!(p, Part::Plan { plan, .. } if plan.starts_with("1. read X")))
    );

    let events = bus.events.lock().unwrap();
    assert!(matches!(
        events.first(),
        Some(AgentEvent::PlanModeExited { plan, .. }) if plan.starts_with("1. read X")
    ));
}

#[tokio::test]
async fn exit_plan_mode_outside_plan_is_noop_but_emits_event() {
    let dir = TempDir::new().unwrap();
    let sid = SessionId::new();
    let store = Arc::new(InMemoryStore::new());
    // Session is on `general`; ExitPlanMode from outside plan mode
    // must NOT call switch_agent (no-op), but must still publish the
    // event with the plan.
    store.seed_session(seed_meta(sid, Some("general"), None));
    let bus = Arc::new(RecordingBus::default());

    let tool = ExitPlanModeTool::new(store.clone());
    let ctx = ctx_with_bus(dir.path(), sid, bus.clone());
    let out = tool
        .run(
            ctx,
            ExitPlanModeInput {
                plan: "naive plan".into(),
            },
        )
        .await
        .unwrap();
    assert!(!out.was_in_plan_mode);
    // Restored value is the fallback `general` so the model sees a
    // sane label even in the no-op path.
    assert_eq!(out.restored_agent, "general");

    // Critical: agent slug did not flip, previous_agent_slug was not
    // mutated (would have been on a real switch).
    let meta = store.get_meta(sid);
    assert_eq!(meta.current_agent_slug.as_deref(), Some("general"));
    assert_eq!(meta.previous_agent_slug, None);

    let events = bus.events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events.first(),
        Some(AgentEvent::PlanModeExited { plan, .. }) if plan == "naive plan"
    ));
}

#[tokio::test]
async fn allowlist_enforced_at_dispatch() {
    use openlet_core::agent::{AgentDefinition, AgentRegistry, AgentSlug, PromptSegments};
    use openlet_core::runtime::agent_allowlist::{partition_by_allowlist, resolve_allowlist};
    use openlet_core::tools::ToolInvocation;
    use std::sync::Arc as StdArc;

    let sid = SessionId::new();
    let concrete = Arc::new(InMemoryStore::new());
    concrete.seed_session(seed_meta(sid, Some("plan"), Some("general")));
    let store: Arc<dyn MemoryStore> = concrete.clone();

    let mut registry = AgentRegistry::new();
    registry
        .insert(AgentDefinition {
            slug: AgentSlug::new("plan").unwrap(),
            title: "plan".into(),
            description: "test".into(),
            prompt_segments: Some(PromptSegments::default()),
            tool_allowlist: vec!["read".into()],
            model_id: "m".into(),
            default_temperature: 0.0,
            context_window: 1000,
            compaction_threshold: 0.8,
            compaction_summary_cap_tokens: 100,
            hidden: false,
        })
        .unwrap();
    let registry = StdArc::new(registry);

    let snap = resolve_allowlist(&store, sid, Some(&registry)).await;
    assert!(snap.is_some(), "expected allowlist snapshot");
    let invs = vec![
        ToolInvocation {
            call_id: "1".into(),
            name: "read".into(),
            args: serde_json::json!({}),
        },
        ToolInvocation {
            call_id: "2".into(),
            name: "write".into(),
            args: serde_json::json!({}),
        },
    ];
    let (allowed, denied) = partition_by_allowlist(&invs, snap.as_ref());
    assert_eq!(allowed.len(), 1);
    assert_eq!(allowed[0].name, "read");
    assert_eq!(denied.len(), 1);
    assert_eq!(denied[0].1.name, "write");
    match &denied[0].2 {
        openlet_core::error::ToolError::NotAllowedInAgent { tool, agent } => {
            assert_eq!(tool, "write");
            assert_eq!(agent, "plan");
        }
        other => panic!("expected NotAllowedInAgent, got {other:?}"),
    }
}

#[tokio::test]
async fn mid_turn_agent_swap_blocks_pending_dispatch() {
    use openlet_core::agent::{AgentDefinition, AgentRegistry, AgentSlug, PromptSegments};
    use openlet_core::runtime::agent_allowlist::{partition_by_allowlist, resolve_allowlist};
    use openlet_core::tools::ToolInvocation;
    use std::sync::Arc as StdArc;

    let sid = SessionId::new();
    let concrete = Arc::new(InMemoryStore::new());
    concrete.seed_session(seed_meta(sid, Some("general"), None));
    let store: Arc<dyn MemoryStore> = concrete.clone();

    let mut registry = AgentRegistry::new();
    registry
        .insert(AgentDefinition {
            slug: AgentSlug::new("general").unwrap(),
            title: "g".into(),
            description: "test".into(),
            prompt_segments: Some(PromptSegments::default()),
            tool_allowlist: vec!["read".into(), "write".into()],
            model_id: "m".into(),
            default_temperature: 0.0,
            context_window: 1000,
            compaction_threshold: 0.8,
            compaction_summary_cap_tokens: 100,
            hidden: false,
        })
        .unwrap();
    registry
        .insert(AgentDefinition {
            slug: AgentSlug::new("plan").unwrap(),
            title: "plan".into(),
            description: "test".into(),
            prompt_segments: Some(PromptSegments::default()),
            tool_allowlist: vec!["read".into()],
            model_id: "m".into(),
            default_temperature: 0.0,
            context_window: 1000,
            compaction_threshold: 0.8,
            compaction_summary_cap_tokens: 100,
            hidden: false,
        })
        .unwrap();
    let registry = StdArc::new(registry);

    // First dispatch under `general`: write is allowed.
    let invs = vec![ToolInvocation {
        call_id: "1".into(),
        name: "write".into(),
        args: serde_json::json!({}),
    }];
    let snap_before = resolve_allowlist(&store, sid, Some(&registry)).await;
    let (allowed_before, denied_before) = partition_by_allowlist(&invs, snap_before.as_ref());
    assert_eq!(allowed_before.len(), 1);
    assert!(denied_before.is_empty());

    // Mid-turn agent swap (simulating EnterPlanMode firing).
    store.switch_agent(sid, "plan").await.unwrap();

    // Second dispatch under `plan`: write must now be denied.
    let snap_after = resolve_allowlist(&store, sid, Some(&registry)).await;
    let (allowed_after, denied_after) = partition_by_allowlist(&invs, snap_after.as_ref());
    assert!(
        allowed_after.is_empty(),
        "write should be denied after swap"
    );
    assert_eq!(denied_after.len(), 1);
    match &denied_after[0].2 {
        openlet_core::error::ToolError::NotAllowedInAgent { tool, agent } => {
            assert_eq!(tool, "write");
            assert_eq!(agent, "plan");
        }
        other => panic!("expected NotAllowedInAgent, got {other:?}"),
    }
}
