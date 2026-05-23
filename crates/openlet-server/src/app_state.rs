//! AppState — shared handles every route accesses.
//!
//! Per amendment §B, AppState commits to `Arc<dyn _>` at the seam. Reasons:
//! ToolCtx already used `Arc<dyn _>`, so the original 6-param generic was
//! buying nothing on the hot path while costing compile time + ergonomics.

use std::collections::HashMap;
use std::sync::Arc;

use dashmap::DashMap;
use openlet_core::adapters::{
    artifact_store::ArtifactStore, event_sink::EventSink, filesystem::Filesystem,
    memory_store::MemoryStore, model_provider::ModelProvider,
    permission_manager::PermissionManager, tool_executor::ToolExecutor,
};
use openlet_core::agent::AgentRegistry;
use openlet_core::config::Config;
use openlet_core::runtime::ConversationRuntime;
use openlet_core::tools::ReadHistory;
use openlet_core::tools::builtins::bash::ShellExecutor;
use openlet_core::tools::registry::ToolRegistry;
use openlet_core::types::agent::{AgentId, AgentSpec};
use openlet_core::types::session::SessionId;
use openlet_plugin_registry::PluginRegistry;
use tokio_util::sync::CancellationToken;

/// Per-session in-flight turn handle. Phase 5 fills in cancellation.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct TurnHandle {
    pub session_id: SessionId,
    pub cancel: CancellationToken,
}

/// Per-agent runtime resources. One agent owns exactly one workspace,
/// so one filesystem and one shell executor — both scoped to that
/// workspace. Cloud impls swap to `CloudFilesystem` / container shell
/// without touching tool code.
#[derive(Clone)]
#[allow(dead_code)]
pub struct AgentResources {
    pub spec: AgentSpec,
    pub fs: Arc<dyn Filesystem>,
    pub shell: Arc<dyn ShellExecutor>,
}

/// Shared application state. Cloneable via interior `Arc`s.
///
/// All event publishing flows through `events: Arc<dyn EventSink>` —
/// Phase 5's two-tier publisher (§G) lives behind that seam, so callers
/// can never bypass persistence by holding a raw broadcast sender.
///
/// `agents` carries one `AgentResources` per registered agent. MVP boot
/// wires a single default agent; cloud plugin populates this map from
/// the user→agent ownership table.
#[derive(Clone)]
#[allow(dead_code)]
pub struct AppState {
    pub provider: Arc<dyn ModelProvider>,
    pub memory: Arc<dyn MemoryStore>,
    pub artifacts: Arc<dyn ArtifactStore>,
    pub tools: Arc<dyn ToolExecutor>,
    pub tool_registry: Arc<ToolRegistry>,
    pub read_histories: Arc<DashMap<SessionId, ReadHistory>>,
    pub events: Arc<dyn EventSink>,
    pub permission: Arc<dyn PermissionManager>,
    pub config: Arc<Config>,
    pub plugin_registry: Arc<PluginRegistry>,
    pub runtime: Arc<ConversationRuntime>,
    pub active_turns: Arc<DashMap<SessionId, TurnHandle>>,
    pub agents: Arc<HashMap<AgentId, AgentResources>>,
    pub default_agent_id: AgentId,
    /// Agent definitions registered by plugins. Indexed by slug; the
    /// HTTP route resolves the per-session slug via `SessionMeta` once
    /// the column lands (phase-08), and falls back to the `general` slug
    /// for MVP.
    pub agent_registry: Arc<AgentRegistry>,
}
