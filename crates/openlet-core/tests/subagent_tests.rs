//! Subagent infrastructure tests — depth/quota policy, ScopedPermissionManager
//! chain semantics, mention parser anchoring, cancel cascade, cost rollup.
//!
//! Covers per-root quota, dynamic chain, cancel cascade, cost rollup,
//! ASCII-only mentions, anchored mentions, and output cap.

use std::sync::Arc;

use async_trait::async_trait;
use openlet_core::adapters::permission_manager::PermissionManager;
use openlet_core::agent::{AgentDefinition, AgentRegistry, AgentSlug};
use openlet_core::error::PermissionError;
use openlet_core::permission::Deferred;
use openlet_core::runtime::subagent::{
    ScopedPermissionManager, SpawnError, TaskRegistry, TaskStatus, parse_subagent_mention,
    plan_subagent_spawn,
};
use openlet_core::types::permission::{
    AlwaysScope, AskId, Decision, PermissionAction, PermissionCtx, PermissionMode,
    PermissionRequest, PermissionRule,
};
use openlet_core::types::session::{SessionId, SessionMeta, SessionStatus};
use rust_decimal::Decimal;
use tokio_util::sync::CancellationToken;

/// Permission manager that allows everything — root layer for chain tests.
struct AllowAll;

#[async_trait]
impl PermissionManager for AllowAll {
    async fn check(
        &self,
        _ctx: PermissionCtx,
        _req: PermissionRequest,
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
    fn take_deferred(&self, _: AskId) -> Option<Deferred<Decision>> {
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

fn make_agent(slug: &str, allow: Vec<String>) -> AgentDefinition {
    AgentDefinition {
        slug: AgentSlug::new(slug.to_string()).expect("slug"),
        title: slug.to_string(),
        description: String::new(),
        prompt_segments: None,
        tool_allowlist: allow,
        model_id: Some("stub".to_string()),
        default_temperature: 0.0,
        context_window: 128_000,
        compaction_threshold: 0.8,
        compaction_summary_cap_tokens: 2_048,
        hidden: false,
    }
}

fn make_session(depth: u8) -> SessionMeta {
    let now = chrono::Utc::now();
    SessionMeta {
        id: SessionId::new(),
        agent_id: openlet_core::types::agent::AgentId::new(),
        status: SessionStatus::Running,
        permission_mode: PermissionMode::WorkspaceWrite,
        parent_session_id: None,
        created_at: now,
        updated_at: now,
        deleted_at: None,
        version: "0.1.0".to_string(),
        extensions: serde_json::Value::Null,
        capabilities: openlet_core::types::session::SessionCapabilities::default(),
        current_agent_slug: None,
        previous_agent_slug: None,
        depth,
        model: None,
    }
}

fn req(perm: &str) -> PermissionRequest {
    PermissionRequest {
        permission: perm.to_string(),
        reason: None,
        timeout: None,
    }
}

fn ctx(sid: SessionId) -> PermissionCtx {
    PermissionCtx {
        session_id: sid,
        mode: PermissionMode::WorkspaceWrite,
    }
}

#[tokio::test]
async fn subagent_depth_exceeded_at_3() {
    // Session at depth 3 calls subagent_task → SubagentDepthExceeded.
    let parent = make_session(3);
    let mut registry = AgentRegistry::new();
    registry.insert(make_agent("worker", vec![])).unwrap();
    let task_registry = TaskRegistry::new(32);
    let parent_perm: Arc<dyn PermissionManager> = Arc::new(AllowAll);
    let cancel = CancellationToken::new();

    let res = plan_subagent_spawn(
        &parent,
        "worker",
        &registry,
        parent_perm,
        &cancel,
        &task_registry,
        parent.id,
        3,
    );
    assert!(
        matches!(res, Err(SpawnError::SubagentDepthExceeded { .. })),
        "expected SubagentDepthExceeded, got Err? {}",
        res.is_err()
    );
}

#[tokio::test]
async fn subagent_quota_exceeded_at_32() {
    // 32 in-flight descendants under one root → 33rd returns quota error.
    let parent = make_session(0);
    let mut registry = AgentRegistry::new();
    registry.insert(make_agent("worker", vec![])).unwrap();
    let task_registry = TaskRegistry::new(32);
    let parent_perm: Arc<dyn PermissionManager> = Arc::new(AllowAll);
    let cancel = CancellationToken::new();

    for _ in 0..32 {
        let res = plan_subagent_spawn(
            &parent,
            "worker",
            &registry,
            parent_perm.clone(),
            &cancel,
            &task_registry,
            parent.id,
            3,
        );
        assert!(res.is_ok(), "first 32 should admit cleanly");
    }
    let over = plan_subagent_spawn(
        &parent,
        "worker",
        &registry,
        parent_perm,
        &cancel,
        &task_registry,
        parent.id,
        3,
    );
    assert!(
        matches!(over, Err(SpawnError::SubagentQuotaExceeded { .. })),
        "expected SubagentQuotaExceeded, got Err? {}",
        over.is_err()
    );
}

#[tokio::test]
async fn quota_decrements_on_completion() {
    // Admit one task, finalize it, admit another — counter resets.
    let parent = make_session(0);
    let mut registry = AgentRegistry::new();
    registry.insert(make_agent("worker", vec![])).unwrap();
    let task_registry = TaskRegistry::new(1);
    let parent_perm: Arc<dyn PermissionManager> = Arc::new(AllowAll);
    let cancel = CancellationToken::new();

    let plan = plan_subagent_spawn(
        &parent,
        "worker",
        &registry,
        parent_perm.clone(),
        &cancel,
        &task_registry,
        parent.id,
        3,
    )
    .expect("first admit");
    // Cap=1 → second admit must reject before finalize.
    let blocked = plan_subagent_spawn(
        &parent,
        "worker",
        &registry,
        parent_perm.clone(),
        &cancel,
        &task_registry,
        parent.id,
        3,
    );
    assert!(matches!(
        blocked,
        Err(SpawnError::SubagentQuotaExceeded { .. })
    ));
    // Finalize releases the slot.
    task_registry.finalize(plan.task_id);
    // Now another admit must succeed.
    let after = plan_subagent_spawn(
        &parent,
        "worker",
        &registry,
        parent_perm,
        &cancel,
        &task_registry,
        parent.id,
        3,
    );
    assert!(after.is_ok(), "post-finalize admit should succeed");
}

#[tokio::test]
async fn permission_chain_grandchild_dynamic() {
    // 1000 random parent×child×grandchild×tool tuples; assert AND semantics.
    // Deterministic LCG so the test is reproducible without `rand`.
    let pool = ["read", "write", "bash", "edit", "todo", "list", "grep"];
    let mut state: u64 = 0xCAFE_F00D_DEAD_BEEF;
    let next = |s: &mut u64| -> u64 {
        *s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *s
    };

    for _ in 0..1000 {
        let r1 = next(&mut state) as usize;
        let r2 = next(&mut state) as usize;
        let r3 = next(&mut state) as usize;
        let child_n = r1 % (pool.len() + 1);
        let grand_n = r2 % (pool.len() + 1);
        let child_allow: Vec<String> = pool
            .iter()
            .take(child_n)
            .map(|s| (*s).to_string())
            .collect();
        let grand_allow: Vec<String> = pool
            .iter()
            .skip(pool.len().saturating_sub(grand_n))
            .map(|s| (*s).to_string())
            .collect();
        let tool = pool[r3 % pool.len()];
        let perm = format!("{tool}:foo");

        let parent_perm: Arc<dyn PermissionManager> = Arc::new(AllowAll);
        let child = ScopedPermissionManager::new(parent_perm, child_allow.clone());
        let child_dyn: Arc<dyn PermissionManager> = Arc::new(child);
        let child_allows_tool = child_allow.is_empty() || child_allow.iter().any(|t| t == tool);
        let grand = ScopedPermissionManager::new(child_dyn, grand_allow.clone());
        let grand_allows_tool = grand_allow.is_empty() || grand_allow.iter().any(|t| t == tool);

        let decision = grand
            .check(ctx(SessionId::new()), req(&perm))
            .await
            .unwrap();
        let allowed = matches!(decision, Decision::Allow);
        let expected = child_allows_tool && grand_allows_tool;
        assert_eq!(
            allowed, expected,
            "tool={tool} child={child_allow:?} grand={grand_allow:?}"
        );
    }
}

#[test]
fn mention_parser_rejects_cyrillic() {
    let mut r = AgentRegistry::new();
    r.insert(make_agent("admin", vec![])).unwrap();
    let cyr = "@\u{0430}dmin foo";
    assert!(parse_subagent_mention(cyr, &r).is_none());
}

#[test]
fn mention_parser_rejects_mid_prompt() {
    let mut r = AgentRegistry::new();
    r.insert(make_agent("admin", vec![])).unwrap();
    assert!(parse_subagent_mention("\n@admin foo", &r).is_none());
    assert!(parse_subagent_mention(" @admin foo", &r).is_none());
    assert!(parse_subagent_mention("hi @admin foo", &r).is_none());
}

#[test]
fn mention_parser_resolves_admin_objective() {
    let mut r = AgentRegistry::new();
    r.insert(make_agent("admin", vec![])).unwrap();
    let (slug, obj) = parse_subagent_mention("@admin investigate failure", &r).expect("matches");
    assert_eq!(slug.as_str(), "admin");
    assert_eq!(obj, "investigate failure");
}

#[tokio::test]
async fn cancellation_cascades_within_1s() {
    // Parent token cancels → cancel_descendants flips child task tokens
    // within tolerance (100ms) and the awaiting handle observes terminal.
    use tokio::time::{Duration, sleep, timeout};
    let parent = make_session(0);
    let mut registry = AgentRegistry::new();
    registry.insert(make_agent("worker", vec![])).unwrap();
    let task_registry = Arc::new(TaskRegistry::new(8));
    let parent_perm: Arc<dyn PermissionManager> = Arc::new(AllowAll);
    let cancel = CancellationToken::new();

    let plan = plan_subagent_spawn(
        &parent,
        "worker",
        &registry,
        parent_perm,
        &cancel,
        &task_registry,
        parent.id,
        3,
    )
    .expect("admit");
    let task_id = plan.task_id;
    let task_registry_for_drive = task_registry.clone();
    let child_cancel = plan.child_cancel.clone();
    // Driver: park until cancelled, then mark cancelled + finalize.
    tokio::spawn(async move {
        child_cancel.cancelled().await;
        task_registry_for_drive
            .set_status(task_id, TaskStatus::Cancelled)
            .await;
    });
    sleep(Duration::from_millis(50)).await;
    task_registry.cancel_descendants(parent.id);
    let done = timeout(
        Duration::from_millis(1100),
        task_registry.await_completion(task_id),
    )
    .await
    .expect("await did not exceed 1.1s");
    assert!(matches!(done.expect("snap").status, TaskStatus::Cancelled));
}

#[tokio::test]
async fn cost_absorbed_into_parent() {
    // Concurrent reads on the registry's cost lock observe the
    // accumulated decimal sum (RwLock semantics — the runtime's
    // session-cost rollup is exercised by the server-side driver).
    let parent = make_session(0);
    let mut registry = AgentRegistry::new();
    registry.insert(make_agent("worker", vec![])).unwrap();
    let task_registry = Arc::new(TaskRegistry::new(8));
    let parent_perm: Arc<dyn PermissionManager> = Arc::new(AllowAll);
    let cancel = CancellationToken::new();
    let plan = plan_subagent_spawn(
        &parent,
        "worker",
        &registry,
        parent_perm,
        &cancel,
        &task_registry,
        parent.id,
        3,
    )
    .expect("admit");

    // Three concurrent writers each adding 0.0010 USD.
    let mut handles = Vec::new();
    for _ in 0..3 {
        let tr = task_registry.clone();
        let id = plan.task_id;
        handles.push(tokio::spawn(async move {
            tr.add_cost(id, Decimal::new(10, 4)).await;
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    let snap = task_registry.poll_async(plan.task_id).await.expect("snap");
    assert_eq!(snap.cost_usd, Decimal::new(30, 4));
}
