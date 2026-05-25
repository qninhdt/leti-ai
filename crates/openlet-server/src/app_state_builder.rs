//! Fluent builder for [`AppState`].
//!
//! `AppStateBuilder` decouples the seven adapter swap points from the
//! handful of derived/default fields. Downstream integrators (cloud
//! binaries) build state with their own `Arc<dyn _>` impls without
//! copying the boot wiring from `main.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use dashmap::DashMap;
use openlet_core::adapters::{
    artifact_store::ArtifactStore, event_sink::EventSink, memory_store::MemoryStore,
    model_provider::ModelProvider, permission_manager::PermissionManager,
    tool_executor::ToolExecutor,
};
use openlet_core::agent::AgentRegistry;
use openlet_core::config::Config;
use openlet_core::runtime::question_registry::QuestionRegistry;
use openlet_core::runtime::{ConversationRuntime, RuntimeConfig};
use openlet_core::tools::ReadHistory;
use openlet_core::tools::registry::ToolRegistry;
use openlet_core::types::agent::AgentId;
use openlet_core::types::session::SessionId;
use openlet_plugin_api::dispatch::HookChains;
use openlet_plugin_registry::PluginRegistry;

use crate::app_state::{AgentResources, AppState, TurnHandle};

/// Errors surfaced when `AppStateBuilder::build` is called with missing
/// required fields.
#[derive(Debug, thiserror::Error)]
pub enum AppStateBuilderError {
    #[error("missing required field: {0}")]
    Missing(&'static str),
}

/// Fluent builder. Use `AppStateBuilder::new()` then chain `.provider(..)`,
/// `.memory(..)`, etc. Required fields must be set before `.build()`;
/// defaults are auto-wired for `read_histories`, `active_turns`,
/// `plugin_registry`, `agent_registry`, and `runtime` (built from
/// provider+memory+events+config when not explicitly supplied).
#[derive(Default)]
pub struct AppStateBuilder {
    provider: Option<Arc<dyn ModelProvider>>,
    memory: Option<Arc<dyn MemoryStore>>,
    artifacts: Option<Arc<dyn ArtifactStore>>,
    tools: Option<Arc<dyn ToolExecutor>>,
    tool_registry: Option<Arc<ToolRegistry>>,
    events: Option<Arc<dyn EventSink>>,
    permission: Option<Arc<dyn PermissionManager>>,
    config: Option<Arc<Config>>,
    plugin_registry: Option<Arc<PluginRegistry>>,
    hook_chains: Option<Arc<HookChains>>,
    runtime: Option<Arc<ConversationRuntime>>,
    read_histories: Option<Arc<DashMap<SessionId, ReadHistory>>>,
    active_turns: Option<Arc<DashMap<SessionId, TurnHandle>>>,
    agents: Option<HashMap<AgentId, AgentResources>>,
    default_agent_id: Option<AgentId>,
    agent_registry: Option<Arc<AgentRegistry>>,
    questions: Option<Arc<QuestionRegistry>>,
}

impl AppStateBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn provider(mut self, v: Arc<dyn ModelProvider>) -> Self {
        self.provider = Some(v);
        self
    }

    #[must_use]
    pub fn memory(mut self, v: Arc<dyn MemoryStore>) -> Self {
        self.memory = Some(v);
        self
    }

    #[must_use]
    pub fn artifacts(mut self, v: Arc<dyn ArtifactStore>) -> Self {
        self.artifacts = Some(v);
        self
    }

    #[must_use]
    pub fn tools(mut self, v: Arc<dyn ToolExecutor>) -> Self {
        self.tools = Some(v);
        self
    }

    #[must_use]
    pub fn tool_registry(mut self, v: Arc<ToolRegistry>) -> Self {
        self.tool_registry = Some(v);
        self
    }

    #[must_use]
    pub fn events(mut self, v: Arc<dyn EventSink>) -> Self {
        self.events = Some(v);
        self
    }

    #[must_use]
    pub fn permission(mut self, v: Arc<dyn PermissionManager>) -> Self {
        self.permission = Some(v);
        self
    }

    #[must_use]
    pub fn config(mut self, v: Arc<Config>) -> Self {
        self.config = Some(v);
        self
    }

    #[must_use]
    pub fn plugin_registry(mut self, v: Arc<PluginRegistry>) -> Self {
        self.plugin_registry = Some(v);
        self
    }

    #[must_use]
    pub fn hook_chains(mut self, v: Arc<HookChains>) -> Self {
        self.hook_chains = Some(v);
        self
    }

    #[must_use]
    pub fn runtime(mut self, v: Arc<ConversationRuntime>) -> Self {
        self.runtime = Some(v);
        self
    }

    #[must_use]
    pub fn read_histories(mut self, v: Arc<DashMap<SessionId, ReadHistory>>) -> Self {
        self.read_histories = Some(v);
        self
    }

    #[must_use]
    pub fn active_turns(mut self, v: Arc<DashMap<SessionId, TurnHandle>>) -> Self {
        self.active_turns = Some(v);
        self
    }

    #[must_use]
    pub fn agents(mut self, v: HashMap<AgentId, AgentResources>) -> Self {
        self.agents = Some(v);
        self
    }

    #[must_use]
    pub fn default_agent_id(mut self, v: AgentId) -> Self {
        self.default_agent_id = Some(v);
        self
    }

    #[must_use]
    pub fn agent_registry(mut self, v: Arc<AgentRegistry>) -> Self {
        self.agent_registry = Some(v);
        self
    }

    #[must_use]
    pub fn questions(mut self, v: Arc<QuestionRegistry>) -> Self {
        self.questions = Some(v);
        self
    }

    /// Validate required fields and assemble the [`AppState`]. Auto-fills
    /// `runtime` from provider+memory+events+config if the integrator did
    /// not supply one explicitly.
    pub fn build(self) -> Result<AppState, AppStateBuilderError> {
        let provider = self
            .provider
            .ok_or(AppStateBuilderError::Missing("provider"))?;
        let memory = self.memory.ok_or(AppStateBuilderError::Missing("memory"))?;
        let artifacts = self
            .artifacts
            .ok_or(AppStateBuilderError::Missing("artifacts"))?;
        let tools = self.tools.ok_or(AppStateBuilderError::Missing("tools"))?;
        let tool_registry = self
            .tool_registry
            .ok_or(AppStateBuilderError::Missing("tool_registry"))?;
        let events = self.events.ok_or(AppStateBuilderError::Missing("events"))?;
        let permission = self
            .permission
            .ok_or(AppStateBuilderError::Missing("permission"))?;
        let config = self.config.ok_or(AppStateBuilderError::Missing("config"))?;
        let agents = self.agents.ok_or(AppStateBuilderError::Missing("agents"))?;
        let default_agent_id = self
            .default_agent_id
            .ok_or(AppStateBuilderError::Missing("default_agent_id"))?;

        let hook_chains = self
            .hook_chains
            .unwrap_or_else(|| Arc::new(openlet_plugin_api::dispatch::HookChains::new()));

        let runtime = self.runtime.unwrap_or_else(|| {
            Arc::new(ConversationRuntime::with_hook_chains(
                provider.clone(),
                memory.clone(),
                events.clone(),
                RuntimeConfig::new(config.default_model.clone()),
                hook_chains.clone(),
            ))
        });

        Ok(AppState {
            provider,
            memory,
            artifacts,
            tools,
            tool_registry,
            read_histories: self
                .read_histories
                .unwrap_or_else(|| Arc::new(DashMap::new())),
            events,
            permission,
            config,
            plugin_registry: self
                .plugin_registry
                .unwrap_or_else(|| Arc::new(PluginRegistry::new())),
            hook_chains,
            runtime,
            active_turns: self
                .active_turns
                .unwrap_or_else(|| Arc::new(DashMap::new())),
            agents: Arc::new(agents),
            default_agent_id,
            agent_registry: self
                .agent_registry
                .unwrap_or_else(|| Arc::new(AgentRegistry::new())),
            questions: self
                .questions
                .unwrap_or_else(|| Arc::new(QuestionRegistry::new())),
        })
    }
}
