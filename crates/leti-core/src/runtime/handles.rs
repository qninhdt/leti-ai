//! Shared adapter handles threaded through the turn loop and tool contexts.
//!
//! `RuntimeHandles` groups the `Arc<dyn _>` adapter seams that every
//! tool call and loop iteration needs. Extracting them into one struct
//! shrinks `LoopContext` and `ToolCtx` construction sites from 14-field
//! splats to a single `.handles.clone()` plus the per-call scalars.

use std::sync::Arc;

use crate::adapters::artifact_store::ArtifactStore;
use crate::adapters::event_sink::EventSink;
use crate::adapters::filesystem::Filesystem;
use crate::adapters::memory_store::MemoryStore;
use crate::adapters::permission_manager::PermissionManager;
use crate::agent::AgentRegistry;
use crate::dispatch::HookChains;
use crate::runtime::question_registry::QuestionRegistry;
use crate::runtime::subagent::TaskRegistry;
use crate::tools::ToolScheduler;
use crate::tools::registry::ToolRegistry;

/// Shared adapter handles passed through the turn loop into every tool
/// dispatch. All fields are `Arc` — cloning is cheap reference-count
/// bumps. Grouping them here eliminates the "god-context" anti-pattern
/// where `LoopContext` and `ToolCtx` each carried 10+ `Arc<dyn _>` fields
/// that were always forwarded together.
#[derive(Clone)]
pub struct RuntimeHandles {
    pub fs: Arc<dyn Filesystem>,
    pub permission: Arc<dyn PermissionManager>,
    pub events: Arc<dyn EventSink>,
    pub artifacts: Arc<dyn ArtifactStore>,
    pub memory: Arc<dyn MemoryStore>,
    pub registry: Arc<ToolRegistry>,
    pub hook_chains: Arc<HookChains>,
    pub questions: Arc<QuestionRegistry>,
    pub task_registry: Arc<TaskRegistry>,
    pub agent_registry: Arc<AgentRegistry>,
    pub tool_scheduler: Arc<ToolScheduler>,
}
