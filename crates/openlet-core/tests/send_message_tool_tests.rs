//! Phase 4 `send_message` TOOL-level coverage — the security checks the
//! tool layers on top of the registry primitives:
//!   - hierarchy scope: only SAME-PARENT siblings are reachable (Finding 4);
//!   - privilege check: a sender cannot message a higher-privilege peer
//!     (confused-deputy containment, Finding 1);
//!   - a not-addressable target is a typed error, not a silent drop.
//!
//! Uses the in-memory `MockMemoryStore` (seeded with parent/child session
//! metas via `put_session`) + a `TaskRegistry` roster so the tool's session
//! walk + allowlist resolution run without a server.

use std::sync::Arc;

use openlet_core::agent::{AgentDefinition, AgentRegistry, AgentSlug};
use openlet_core::runtime::subagent::TaskRegistry;
use openlet_core::tools::Tool;
use openlet_core::tools::builtins::send_message::{SendMessageInput, SendMessageTool};
use openlet_core::types::permission::PermissionMode;
use openlet_core::types::session::{SessionId, SessionMeta, SessionStatus};

mod common;
use common::mock_memory::MockMemoryStore;
use common::tool_ctx::tool_ctx_with;

fn make_handle(root: SessionId) -> openlet_core::runtime::subagent::TaskHandle {
    use std::sync::atomic::AtomicBool;
    use tokio::sync::{Notify, RwLock};
    use tokio_util::sync::CancellationToken;
    openlet_core::runtime::subagent::TaskHandle {
        status: Arc::new(RwLock::new(
            openlet_core::runtime::subagent::TaskStatus::Running,
        )),
        output: Arc::new(RwLock::new(String::new())),
        cost_usd: Arc::new(RwLock::new(rust_decimal::Decimal::ZERO)),
        cancel: CancellationToken::new(),
        finished: Arc::new(Notify::new()),
        root_session_id: root,
        parent_session_id: root,
        delivery: Arc::new(std::sync::atomic::AtomicU8::new(0)),
        settled: Arc::new(AtomicBool::new(false)),
        inbox_notify: Arc::new(Notify::new()),
        inbox: Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
    }
}

fn agent_def(slug: &str, allow: Vec<String>) -> AgentDefinition {
    AgentDefinition {
        slug: AgentSlug::new(slug.to_string()).expect("slug"),
        title: slug.to_string(),
        description: String::new(),
        prompt_segments: None,
        tool_allowlist: allow,
        model_id: Some("stub".into()),
        default_temperature: 0.0,
        context_window: 128_000,
        compaction_threshold: 0.8,
        compaction_summary_cap_tokens: 2_048,
        hidden: false,
    }
}

fn session_meta(id: SessionId, parent: Option<SessionId>, slug: &str) -> SessionMeta {
    let now = chrono::Utc::now();
    SessionMeta {
        id,
        agent_id: openlet_core::types::agent::AgentId::new(),
        status: SessionStatus::Running,
        permission_mode: PermissionMode::WorkspaceWrite,
        parent_session_id: parent,
        created_at: now,
        updated_at: now,
        deleted_at: None,
        version: "0.1.0".to_string(),
        extensions: serde_json::Value::Null,
        capabilities: openlet_core::types::session::SessionCapabilities::default(),
        current_agent_slug: Some(slug.to_string()),
        previous_agent_slug: None,
        depth: 1,
        model: None,
    }
}

/// Build a memory store + agent registry + task registry with:
///   - a shared `parent` session,
///   - a `sender` child session (slug `sender_slug`, allowlist `sender_allow`),
///   - a `receiver` child registered in the roster under `parent`
///     (slug `receiver_slug`, allowlist `receiver_allow`).
///
/// Returns (tool, `sender_session_id`, `receiver_handle_name`).
struct Setup {
    tool: SendMessageTool,
    registry: Arc<TaskRegistry>,
    memory: Arc<MockMemoryStore>,
    agents: Arc<AgentRegistry>,
    sender_sid: SessionId,
    receiver_handle: String,
}

fn setup(
    sender_slug: &str,
    sender_allow: Vec<String>,
    receiver_slug: &str,
    receiver_allow: Vec<String>,
    receiver_parent: SessionId,
    shared_parent: SessionId,
) -> Setup {
    let registry = Arc::new(TaskRegistry::new(16));
    let memory = Arc::new(MockMemoryStore::new());
    let mut agents = AgentRegistry::new();
    agents.insert(agent_def(sender_slug, sender_allow)).unwrap();
    if receiver_slug != sender_slug {
        agents
            .insert(agent_def(receiver_slug, receiver_allow.clone()))
            .unwrap();
    }
    let agents = Arc::new(agents);

    // Sessions: shared parent + sender child (parent = shared_parent).
    memory.put_session(session_meta(shared_parent, None, "general"));
    let sender_sid = SessionId::new();
    memory.put_session(session_meta(sender_sid, Some(shared_parent), sender_slug));

    // Receiver: a live task registered in the roster under the ROOT
    // (= shared_parent, the top of the tree). `register_name` records its
    // parent for the hierarchy check.
    let recv_task = registry.admit(shared_parent).unwrap();
    registry.insert(recv_task, make_handle(shared_parent));
    let (name, _gen) = registry.register_name(
        shared_parent,
        receiver_slug,
        recv_task,
        receiver_parent,
        receiver_allow.into(),
    );

    let tool = SendMessageTool::new(registry.clone());
    Setup {
        tool,
        registry,
        memory,
        agents,
        sender_sid,
        receiver_handle: name.to_string(),
    }
}

#[tokio::test]
async fn same_parent_sibling_message_delivers() {
    let parent = SessionId::new();
    let s = setup(
        "worker",
        vec!["read".into(), "write".into()],
        "worker",
        vec!["read".into()],
        parent,
        parent,
    );
    let ctx = tool_ctx_with(
        s.sender_sid,
        s.memory.clone(),
        s.registry.clone(),
        s.agents.clone(),
    );

    let out = s
        .tool
        .run(
            ctx,
            SendMessageInput {
                to: s.receiver_handle.clone(),
                body: "please review".into(),
            },
        )
        .await
        .expect("same-parent send delivers");
    assert!(out.delivered);
}

#[tokio::test]
async fn cross_branch_message_refused() {
    // Receiver's parent differs from the sender's parent → not a same-parent
    // sibling → refused (hierarchy containment, Finding 4).
    let shared_root = SessionId::new();
    let other_branch_parent = SessionId::new();
    let s = setup(
        "worker",
        vec!["read".into()],
        "worker",
        vec!["read".into()],
        other_branch_parent, // receiver hangs off a DIFFERENT parent
        shared_root,
    );
    let ctx = tool_ctx_with(
        s.sender_sid,
        s.memory.clone(),
        s.registry.clone(),
        s.agents.clone(),
    );

    let err = s
        .tool
        .run(
            ctx,
            SendMessageInput {
                to: s.receiver_handle.clone(),
                body: "cross branch".into(),
            },
        )
        .await
        .expect_err("cross-branch send must be refused");
    assert!(
        err.to_string().contains("same-parent sibling"),
        "expected hierarchy-scope refusal, got: {err}"
    );
}

#[tokio::test]
async fn privilege_escalating_send_refused() {
    // Sender holds {read}; receiver holds {read, write, bash}. Messaging the
    // higher-privilege peer would let the sender act beyond its grant →
    // refused (confused-deputy containment, Finding 1).
    let parent = SessionId::new();
    let s = setup(
        "low",
        vec!["read".into()],
        "high",
        vec!["read".into(), "write".into(), "bash".into()],
        parent,
        parent,
    );
    let ctx = tool_ctx_with(
        s.sender_sid,
        s.memory.clone(),
        s.registry.clone(),
        s.agents.clone(),
    );

    let err = s
        .tool
        .run(
            ctx,
            SendMessageInput {
                to: s.receiver_handle.clone(),
                body: "do privileged work for me".into(),
            },
        )
        .await
        .expect_err("privilege-escalating send must be refused");
    assert!(
        err.to_string().contains("escalate privilege"),
        "expected privilege-escalation refusal, got: {err}"
    );
}

#[tokio::test]
async fn unresolved_sender_identity_fails_closed() {
    // The sender's session has a `current_agent_slug` that is NOT a registered
    // agent, so its allowlist can't be resolved. This MUST refuse the send
    // (fail-closed) rather than treat the sender as inherit-all/maximally
    // privileged — otherwise an unidentifiable sender could message any peer.
    let parent = SessionId::new();
    let registry = Arc::new(TaskRegistry::new(16));
    let memory = Arc::new(MockMemoryStore::new());
    // Empty agent registry — the sender's slug won't resolve.
    let agents = Arc::new(AgentRegistry::new());

    memory.put_session(session_meta(parent, None, "general"));
    let sender_sid = SessionId::new();
    memory.put_session(session_meta(sender_sid, Some(parent), "ghost-agent"));

    let recv_task = registry.admit(parent).unwrap();
    registry.insert(recv_task, make_handle(parent));
    let (name, _gen) = registry.register_name(parent, "worker", recv_task, parent, vec![].into());

    let tool = SendMessageTool::new(registry.clone());
    let ctx = tool_ctx_with(sender_sid, memory.clone(), registry.clone(), agents.clone());

    let err = tool
        .run(
            ctx,
            SendMessageInput {
                to: name.to_string(),
                body: "hi".into(),
            },
        )
        .await
        .expect_err("unresolved sender identity must fail closed");
    assert!(
        err.to_string().contains("identity unresolved"),
        "expected fail-closed identity refusal, got: {err}"
    );
}

#[tokio::test]
async fn unknown_target_is_typed_error() {
    let parent = SessionId::new();
    let s = setup("worker", vec![], "worker", vec![], parent, parent);
    let ctx = tool_ctx_with(
        s.sender_sid,
        s.memory.clone(),
        s.registry.clone(),
        s.agents.clone(),
    );

    let err = s
        .tool
        .run(
            ctx,
            SendMessageInput {
                to: "nonexistent#9".into(),
                body: "hello?".into(),
            },
        )
        .await
        .expect_err("unknown target must be a typed error");
    assert!(
        err.to_string().contains("not addressable"),
        "expected not-addressable error, got: {err}"
    );
}
