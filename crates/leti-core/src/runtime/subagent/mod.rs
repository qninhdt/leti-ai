//! In-process nested subagent runtime.
//!
//! Wires the four building blocks:
//!   - [`scoped_permissions::ScopedPermissionManager`] — dynamic chain
//!     filtering for the child agent's tool allowlist.
//!   - [`task_registry::TaskRegistry`] — handle store, depth/quota
//!     bookkeeping, output cap, cost rollup.
//!   - [`mention_parser::parse_subagent_mention`] — `@subagent_name`
//!     prompt routing, ASCII-only.
//!   - [`spawn_subagent_session`] — admit a `TaskId`, build a child
//!     `SessionMeta`, attach a child cancellation token, drive a nested
//!     turn loop on a fresh tokio task.
//!
//! TUI-side rendering of nested progress is sibling work; this module
//! only emits SSE `subagent.*` frames the renderer can consume.

pub mod mention_parser;
pub mod scoped_permissions;
pub mod task_registry;
pub mod task_types;

pub use mention_parser::parse_subagent_mention;
pub use scoped_permissions::ScopedPermissionManager;
pub use task_registry::{BackgroundTransition, TaskRegistry};
pub use task_types::{
    DEFAULT_MAX_DEPTH, DEFAULT_MAX_LIFETIME_SPAWNS, DEFAULT_MAX_PER_SESSION, DeliveryOwnership,
    HandleName, InboxMessage, MAX_OUTPUT_BYTES, RosterEntry, SpawnError, SubagentExecution,
    SubagentExecutionStatus, TaskHandle, TaskId, TaskSnapshot, TaskStatus,
};

use std::sync::Arc;

use crate::adapters::permission_manager::PermissionManager;
use crate::agent::{AgentRegistry, AgentSlug};
use crate::types::session::{SessionId, SessionMeta, SessionStatus};
use rust_decimal::Decimal;
use tokio::sync::{Notify, RwLock};
use tokio_util::sync::CancellationToken;

/// Resolved subagent spawn context — the runtime hands this to the
/// caller in lieu of the full session+driver wiring (which lives in the
/// server crate where `AppState` exists). Keeps `leti-core` free of
/// HTTP/route deps while still owning depth/quota policy.
pub struct SpawnPlan {
    pub task_id: TaskId,
    pub child: SessionMeta,
    pub parent_meta: SessionMeta,
    pub agent_slug: AgentSlug,
    pub child_perm: Arc<dyn PermissionManager>,
    pub child_cancel: CancellationToken,
    pub handle: TaskHandle,
    /// Unique roster handle name assigned to this child (Phase 4). The
    /// driver removes the roster entry under this name when the task is no
    /// longer background-alive.
    pub handle_name: HandleName,
    /// Roster generation at registration — bumped on any later rebind so a
    /// stale `send_message` snapshot is refused (name-safety gen-check).
    pub roster_gen: u64,
}

/// Resolve `subagent_slug` and admit the spawn. Returns a [`SpawnPlan`]
/// the server-side driver consumes to launch a nested
/// `ConversationRuntime::run_loop`. Decrements quota on every error
/// path so a failed admit doesn't poison the per-root counter.
///
/// Depth + quota enforcement:
///   - depth check happens BEFORE quota admit (cheap reject for over-deep).
///   - quota admit is AcqRel CAS so racing siblings can't both pass.
///   - on slug-not-found, quota is released before the `Err` returns.
#[allow(clippy::too_many_arguments)]
pub fn plan_subagent_spawn(
    parent: &SessionMeta,
    subagent_slug: &str,
    agents: &AgentRegistry,
    parent_perm: Arc<dyn PermissionManager>,
    parent_cancel: &CancellationToken,
    registry: &TaskRegistry,
    root_session_id: SessionId,
    max_depth: u8,
) -> Result<SpawnPlan, SpawnError> {
    let next_depth = parent.depth.saturating_add(1);
    if next_depth > max_depth {
        return Err(SpawnError::SubagentDepthExceeded {
            requested: next_depth,
            max: max_depth,
        });
    }

    let task_id = registry.admit(root_session_id)?;

    let available = || {
        let mut slugs = agents
            .iter_visible()
            .map(|(slug, _)| slug.as_str().to_string())
            .collect::<Vec<_>>();
        slugs.sort();
        if slugs.is_empty() {
            "none".to_string()
        } else {
            slugs.join(", ")
        }
    };
    let not_found = || {
        SpawnError::SubagentTypeNotFound(format!(
            "{subagent_slug}. Available agent types: {}. Use an exact listed slug or omit subagent_type to use general",
            available()
        ))
    };
    let slug = AgentSlug::new(subagent_slug.to_string()).map_err(|_| {
        registry.release_quota(root_session_id);
        not_found()
    })?;
    let child_def = match agents.get(&slug) {
        Some(d) => d.clone(),
        None => {
            registry.release_quota(root_session_id);
            return Err(not_found());
        }
    };

    let child_id = SessionId::new();
    let now = chrono::Utc::now();
    let child = SessionMeta {
        id: child_id,
        agent_id: parent.agent_id,
        status: SessionStatus::Running,
        permission_mode: parent.permission_mode,
        interaction_mode: parent.interaction_mode,
        parent_session_id: Some(parent.id),
        created_at: now,
        updated_at: now,
        deleted_at: None,
        version: parent.version.clone(),
        extensions: parent.extensions.clone(),
        capabilities: parent.capabilities,
        current_agent_slug: parent.current_agent_slug.clone(),
        previous_agent_slug: parent.previous_agent_slug.clone(),
        depth: next_depth,
        model: parent.model.clone(),
    };

    let child_perm: Arc<dyn PermissionManager> = Arc::new(ScopedPermissionManager::new(
        parent_perm,
        child_def.tool_allowlist.clone(),
    ));
    let child_cancel = parent_cancel.child_token();

    let handle = TaskHandle {
        status: Arc::new(RwLock::new(TaskStatus::Running)),
        output: Arc::new(RwLock::new(String::new())),
        cost_usd: Arc::new(RwLock::new(Decimal::ZERO)),
        cancel: child_cancel.clone(),
        finished: Arc::new(Notify::new()),
        root_session_id,
        parent_session_id: parent.id,
        delivery: Arc::new(std::sync::atomic::AtomicU8::new(
            DeliveryOwnership::ForegroundWaiting.as_u8(),
        )),
        settled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        inbox: Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
        inbox_notify: Arc::new(Notify::new()),
    };
    registry.insert(task_id, handle.clone());
    registry.link_child(task_id, child_id);

    // Register the child in the sibling roster under its ROOT session with
    // a unique handle name (auto-suffixed on same-slug collision). The
    // assigned name + generation ride the SpawnPlan so the driver can
    // remove the entry when the task is no longer background-alive, and so
    // a `send_message` snapshot can gen-check against a later rebind.
    let (handle_name, roster_gen) = registry.register_name(
        root_session_id,
        subagent_slug,
        task_id,
        parent.id,
        child_def.tool_allowlist.clone().into(),
    );

    Ok(SpawnPlan {
        task_id,
        child,
        parent_meta: parent.clone(),
        agent_slug: slug,
        child_perm,
        child_cancel,
        handle,
        handle_name,
        roster_gen,
    })
}

/// Admit another execution against an existing child session. This is the
/// continuation primitive: the child session remains the agent identity and
/// transcript, while the new task id represents a fresh, independently
/// cancellable run.
#[allow(clippy::too_many_arguments)]
pub fn plan_subagent_continuation(
    child: &SessionMeta,
    subagent_slug: &str,
    agents: &AgentRegistry,
    parent_perm: Arc<dyn PermissionManager>,
    parent_cancel: &CancellationToken,
    registry: &TaskRegistry,
    root_session_id: SessionId,
    max_depth: u8,
) -> Result<SpawnPlan, SpawnError> {
    if child.depth == 0 || child.depth > max_depth {
        return Err(SpawnError::SubagentDepthExceeded {
            requested: child.depth,
            max: max_depth,
        });
    }
    let task_id = registry.admit(root_session_id)?;
    let slug = AgentSlug::new(subagent_slug.to_string()).map_err(|_| {
        registry.release_quota(root_session_id);
        SpawnError::SubagentTypeNotFound(subagent_slug.to_string())
    })?;
    let Some(child_def) = agents.get(&slug).cloned() else {
        registry.release_quota(root_session_id);
        return Err(SpawnError::SubagentTypeNotFound(subagent_slug.to_string()));
    };
    let parent_session_id = child.parent_session_id.unwrap_or(root_session_id);
    let child_perm: Arc<dyn PermissionManager> = Arc::new(ScopedPermissionManager::new(
        parent_perm,
        child_def.tool_allowlist.clone(),
    ));
    let child_cancel = parent_cancel.child_token();
    let handle = TaskHandle {
        status: Arc::new(RwLock::new(TaskStatus::Running)),
        output: Arc::new(RwLock::new(String::new())),
        cost_usd: Arc::new(RwLock::new(Decimal::ZERO)),
        cancel: child_cancel.clone(),
        finished: Arc::new(Notify::new()),
        root_session_id,
        parent_session_id,
        delivery: Arc::new(std::sync::atomic::AtomicU8::new(
            DeliveryOwnership::ForegroundWaiting.as_u8(),
        )),
        settled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        inbox: Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new())),
        inbox_notify: Arc::new(Notify::new()),
    };
    registry.insert(task_id, handle.clone());
    registry.link_child(task_id, child.id);
    let (handle_name, roster_gen) = registry.register_name(
        root_session_id,
        subagent_slug,
        task_id,
        parent_session_id,
        child_def.tool_allowlist.clone().into(),
    );
    Ok(SpawnPlan {
        task_id,
        child: child.clone(),
        parent_meta: child.clone(),
        agent_slug: slug,
        child_perm,
        child_cancel,
        handle,
        handle_name,
        roster_gen,
    })
}
