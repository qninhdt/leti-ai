//! AppState — shared handles every route accesses.
//!
//! Per amendment §B, AppState commits to `Arc<dyn _>` at the seam. Reasons:
//! ToolCtx already used `Arc<dyn _>`, so the original 6-param generic was
//! buying nothing on the hot path while costing compile time + ergonomics.

use std::sync::Arc;

use dashmap::DashMap;
use openlet_core::adapters::{
    artifact_store::ArtifactStore, event_sink::EventSink, memory_store::MemoryStore,
    model_provider::ModelProvider, permission_manager::PermissionManager, tool_executor::ToolExecutor,
};
use openlet_core::config::Config;
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

/// Shared application state. Cloneable via interior `Arc`s.
///
/// All event publishing flows through `events: Arc<dyn EventSink>` —
/// Phase 5's two-tier publisher (§G) lives behind that seam, so callers
/// can never bypass persistence by holding a raw broadcast sender.
///
/// Fields beyond `config` are dormant in Phase 1 (only the health route
/// runs); Phase 2+ wires them through. The `dead_code` allow keeps the
/// scaffolding compiling cleanly under clippy `-D warnings`.
#[derive(Clone)]
#[allow(dead_code)]
pub struct AppState {
    pub provider: Arc<dyn ModelProvider>,
    pub memory: Arc<dyn MemoryStore>,
    pub artifacts: Arc<dyn ArtifactStore>,
    pub tools: Arc<dyn ToolExecutor>,
    pub events: Arc<dyn EventSink>,
    pub permission: Arc<dyn PermissionManager>,
    pub config: Arc<Config>,
    pub plugin_registry: Arc<PluginRegistry>,
    pub active_turns: Arc<DashMap<SessionId, TurnHandle>>,
}
