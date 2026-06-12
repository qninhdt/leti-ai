//! AppState — shared handles every route accesses.
//!
//! AppState commits to `Arc<dyn _>` at the seam. Reasons:
//! ToolCtx already used `Arc<dyn _>`, so the original 6-param generic was
//! buying nothing on the hot path while costing compile time + ergonomics.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use dashmap::DashMap;
use openlet_core::adapters::{
    artifact_store::ArtifactStore, event_sink::EventSink, filesystem::Filesystem,
    memory_store::MemoryStore, model_provider::ModelProvider,
    permission_manager::PermissionManager,
};
use openlet_core::agent::AgentRegistry;
use openlet_core::config::Config;
use openlet_core::runtime::ConversationRuntime;
use openlet_core::runtime::question_registry::QuestionRegistry;
use openlet_core::runtime::subagent::TaskRegistry;
use openlet_core::tools::ReadHistory;
use openlet_core::tools::builtins::bash::ShellExecutor;
use openlet_core::tools::registry::ToolRegistry;
use openlet_core::types::agent::{AgentId, AgentSpec};
use openlet_core::types::session::SessionId;
use openlet_plugin_api::dispatch::HookChains;
use openlet_plugin_registry::PluginHandles;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

/// Per-session in-flight turn handle. Cancellation invariants:
/// - `request_cancel` uses CompareAndSwap so concurrent cancellers (HTTP
///   abort + DELETE + plugin `cancel_session`) emit exactly one
///   `Cancelling` status event.
/// - `exited` is signalled by the driving task in its Drop guard; DELETE
///   awaits it before marking the session terminal so the LLM can't keep
///   streaming on a session the client thinks is gone.
/// - The driving task removes its OWN slot via `remove_if(Arc::ptr_eq)`
///   so a stale finalizer can't stomp a fresh turn's status.
#[derive(Clone)]
pub struct TurnHandle {
    pub session_id: SessionId,
    pub cancel: CancellationToken,
    /// CAS gate so exactly one canceller publishes `Cancelling` and
    /// trips `cancel`. Subsequent calls observe `false` and no-op.
    pub cancel_emitted: Arc<AtomicBool>,
    /// Notified by the driving task in its Drop guard on exit (success,
    /// error, OR panic). DELETE/abort awaiters resolve immediately.
    pub exited: Arc<Notify>,
}

impl std::fmt::Debug for TurnHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnHandle")
            .field("session_id", &self.session_id)
            .field(
                "cancel_emitted",
                &self.cancel_emitted.load(Ordering::Acquire),
            )
            .finish()
    }
}

impl TurnHandle {
    pub fn new(session_id: SessionId) -> Self {
        Self {
            session_id,
            cancel: CancellationToken::new(),
            cancel_emitted: Arc::new(AtomicBool::new(false)),
            exited: Arc::new(Notify::new()),
        }
    }

    /// Returns `true` if THIS call is the one that flipped the flag —
    /// caller is responsible for `cancel.cancel()` and the `Cancelling`
    /// event. Subsequent callers get `false` and must no-op.
    pub fn request_cancel(&self) -> bool {
        self.cancel_emitted
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

/// Per-agent runtime resources. One agent owns exactly one workspace,
/// so one filesystem and one shell executor — both scoped to that
/// workspace. Cloud impls swap to `CloudFilesystem` / container shell
/// without touching tool code.
#[derive(Clone)]
pub struct AgentResources {
    pub spec: AgentSpec,
    pub fs: Arc<dyn Filesystem>,
    pub shell: Arc<dyn ShellExecutor>,
}

/// Shared application state. Cloneable via interior `Arc`s.
///
/// All event publishing flows through `events: Arc<dyn EventSink>` —
/// the two-tier publisher lives behind that seam, so callers
/// can never bypass persistence by holding a raw broadcast sender.
///
/// `agents` carries one `AgentResources` per registered agent. MVP boot
/// wires a single default agent; cloud plugin populates this map from
/// the user→agent ownership table.
#[derive(Clone)]
pub struct AppState {
    pub provider: Arc<dyn ModelProvider>,
    pub memory: Arc<dyn MemoryStore>,
    pub artifacts: Arc<dyn ArtifactStore>,
    pub tool_registry: Arc<ToolRegistry>,
    pub read_histories: Arc<DashMap<SessionId, ReadHistory>>,
    pub events: Arc<dyn EventSink>,
    pub permission: Arc<dyn PermissionManager>,
    pub config: Arc<Config>,
    pub plugin_registry: Arc<PluginHandles>,
    pub hook_chains: Arc<HookChains>,
    pub runtime: Arc<ConversationRuntime>,
    pub active_turns: Arc<DashMap<SessionId, TurnHandle>>,
    pub agents: Arc<HashMap<AgentId, AgentResources>>,
    pub default_agent_id: AgentId,
    /// Absolute path of the agent workspace root. The `Filesystem` trait
    /// object jails reads to this root but does not expose it; routes that
    /// must convert the trait's absolute `glob` results back to
    /// workspace-relative paths (the `/v1/files` listing) read it here.
    pub workspace_root: std::path::PathBuf,
    /// Agent definitions registered by plugins. Indexed by slug; the
    /// HTTP route resolves the per-session slug via `SessionMeta` once
    /// the column lands, and falls back to the `general` slug
    /// for MVP.
    pub agent_registry: Arc<AgentRegistry>,
    /// In-flight `ask_user` rendezvous map. Tools register oneshots
    /// here while suspended; `POST /v1/session/:id/question/answer`
    /// resolves them.
    pub questions: Arc<QuestionRegistry>,
    /// In-process subagent task registry. Bounded by
    /// `OPENLET_SUBAGENT_MAX_PER_SESSION` (default 32) per ROOT session.
    pub task_registry: Arc<TaskRegistry>,
}

impl AppState {
    /// Look up a session or return a 404 `AppError`. Centralises the
    /// `get_session(sid)? .ok_or_else(...)` shape used by every route
    /// that takes a `:id` path param.
    pub async fn require_session(
        &self,
        sid: SessionId,
    ) -> Result<openlet_core::types::session::SessionMeta, crate::error::AppError> {
        self.memory
            .get_session(sid)
            .await?
            .ok_or_else(|| crate::error::AppError::not_found("session_not_found", "Not found"))
    }

    /// Atomically request cancellation of an in-flight turn for `sid`.
    /// Returns `true` if THIS call was the one that flipped the CAS gate
    /// (and thus emitted the `Cancelling` status); subsequent callers see
    /// `false` and no-op. Idempotent across HTTP DELETE, abort, and the
    /// plugin `cancel_session` core_api path.
    pub async fn try_cancel_active_turn(&self, sid: SessionId) -> bool {
        let Some(handle) = self.active_turns.get(&sid).map(|h| h.clone()) else {
            return false;
        };
        if !handle.request_cancel() {
            return false;
        }
        handle.cancel.cancel();
        crate::events::publish_status(
            &self.events,
            sid,
            openlet_core::types::session::SessionStatus::Cancelling,
        )
        .await;
        true
    }
}
