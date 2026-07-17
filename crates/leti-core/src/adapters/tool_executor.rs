use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::agent::AgentRegistry;
use crate::runtime::TurnExtensions;
use crate::runtime::question_registry::QuestionRegistry;
use crate::runtime::subagent::TaskRegistry;
use crate::tools::read_history::ReadHistory;
use crate::types::agent::AgentId;
use crate::types::message::MessageId;
use crate::types::permission::PermissionMode;
use crate::types::session::SessionId;

use super::artifact_store::ArtifactStore;
use super::event_sink::EventSink;
use super::filesystem::Filesystem;
use super::memory_store::MemoryStore;
use super::permission_manager::PermissionManager;

/// Per-call context carrying handles a tool needs to enforce permissions
/// and emit events. `ToolCtx` uses `Arc<dyn _>` handles throughout. The
/// filesystem is itself an `Arc<dyn Filesystem>` — built-in
/// file tools (`read`/`write`/`edit`/`list`/`glob`/`grep`) call
/// `ctx.fs.*` so a cloud impl can swap the workspace backing without
/// touching tool code.
#[derive(Clone)]
pub struct ToolCtx {
    /// Opaque host data carried for this turn; the engine never interprets it.
    pub ext: TurnExtensions,
    pub session_id: SessionId,
    pub agent_id: AgentId,
    pub message_id: MessageId,
    pub call_id: String,
    pub mode: PermissionMode,
    pub fs: Arc<dyn Filesystem>,
    pub permission: Arc<dyn PermissionManager>,
    pub events: Arc<dyn EventSink>,
    pub artifacts: Arc<dyn ArtifactStore>,
    pub read_history: ReadHistory,
    pub cancel: CancellationToken,
    /// In-flight `ask_user` rendezvous map. The `ask_user` tool registers
    /// a oneshot here at run-time; the REST handler resolves the entry on
    /// the matching `POST /v1/session/:id/question/answer` reply.
    pub questions: Arc<QuestionRegistry>,
    /// Memory-store handle. Tools that need to inspect session-level
    /// state (capabilities, extensions, permission mode) read through
    /// this — kept on `ToolCtx` so the runtime stays the only authority
    /// on which adapter implementation backs the lookup.
    pub memory: Arc<dyn MemoryStore>,
    /// In-process subagent task registry. Tools that spawn nested
    /// agents (`subagent_task`, `task_status`) consult this to admit /
    /// poll / cancel descendants.
    pub task_registry: Arc<TaskRegistry>,
    /// Agent definitions resolved by slug. Subagent spawn validates
    /// `subagent_type` against this registry; a slug not found here
    /// returns `subagent_type_not_found` without admitting quota.
    pub agent_registry: Arc<AgentRegistry>,
}
