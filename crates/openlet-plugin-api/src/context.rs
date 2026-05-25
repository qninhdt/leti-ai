//! [`PluginContext`] — registration API exposed during a plugin's
//! `install` call.
//!
//! Plugins register agents, tools, providers, and per-kind hook
//! handlers. The host drains the context after install completes via
//! [`PluginContext::into_registrations`] and merges the result into the
//! shared [`HookChains`] + tool registry.

use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;
use openlet_core::adapters::model_provider::ModelProvider;
use openlet_core::agent::AgentDefinition;
use openlet_core::tools::ToolHandle;
use serde::de::DeserializeOwned;

use crate::dispatch::{HookChains, HookEntry, HookFuture};
use crate::hooks::{
    HookKind, HookResult, Priority,
    io::{
        AfterToolCallCtx, AfterTurnCtx, BeforeToolCallCtx, BeforeTurnCtx, OnChatHeadersCtx,
        OnChatMessagesCtx, OnChatParamsCtx, OnCompactionCtx, OnCostTickCtx, OnEventCtx,
        OnMessageCtx, OnPermissionAskCtx, OnSessionStatusCtx, OnStepFinishCtx,
    },
};
use crate::manifest::{Capability, PluginManifest};
use crate::plugin::PluginError;

/// Registration API exposed to plugins during `install`.
pub struct PluginContext {
    manifest: PluginManifest,
    raw_config: serde_json::Value,
    core_api: Arc<dyn CoreApi>,
    registered_agents: Vec<AgentDefinition>,
    registered_tools: Vec<ToolHandle>,
    registered_provider: Option<Arc<dyn ModelProvider>>,
    next_index: usize,
    pub(crate) before_turn: Vec<HookEntry<BeforeTurnCtx>>,
    pub(crate) after_turn: Vec<HookEntry<AfterTurnCtx>>,
    pub(crate) on_chat_params: Vec<HookEntry<OnChatParamsCtx>>,
    pub(crate) on_chat_messages: Vec<HookEntry<OnChatMessagesCtx>>,
    pub(crate) on_chat_headers: Vec<HookEntry<OnChatHeadersCtx>>,
    pub(crate) before_tool_call: Vec<HookEntry<BeforeToolCallCtx>>,
    pub(crate) after_tool_call: Vec<HookEntry<AfterToolCallCtx>>,
    pub(crate) on_permission_ask: Vec<HookEntry<OnPermissionAskCtx>>,
    pub(crate) on_message: Vec<HookEntry<OnMessageCtx>>,
    pub(crate) on_cost_tick: Vec<HookEntry<OnCostTickCtx>>,
    pub(crate) on_step_finish: Vec<HookEntry<OnStepFinishCtx>>,
    pub(crate) on_compaction: Vec<HookEntry<OnCompactionCtx>>,
    pub(crate) on_session_status: Vec<HookEntry<OnSessionStatusCtx>>,
    pub(crate) on_event: Vec<HookEntry<OnEventCtx>>,
}

/// Drained registrations from a `PluginContext`. The host merges every
/// chain into the global [`HookChains`] then calls `sort_all`.
pub struct PluginRegistrations {
    pub agents: Vec<AgentDefinition>,
    pub tools: Vec<ToolHandle>,
    pub provider: Option<Arc<dyn ModelProvider>>,
    pub chains: HookChains,
}

impl PluginContext {
    #[must_use]
    pub fn new(
        manifest: PluginManifest,
        raw_config: serde_json::Value,
        core_api: Arc<dyn CoreApi>,
    ) -> Self {
        Self {
            manifest,
            raw_config,
            core_api,
            registered_agents: Vec::new(),
            registered_tools: Vec::new(),
            registered_provider: None,
            next_index: 0,
            before_turn: Vec::new(),
            after_turn: Vec::new(),
            on_chat_params: Vec::new(),
            on_chat_messages: Vec::new(),
            on_chat_headers: Vec::new(),
            before_tool_call: Vec::new(),
            after_tool_call: Vec::new(),
            on_permission_ask: Vec::new(),
            on_message: Vec::new(),
            on_cost_tick: Vec::new(),
            on_step_finish: Vec::new(),
            on_compaction: Vec::new(),
            on_session_status: Vec::new(),
            on_event: Vec::new(),
        }
    }

    #[must_use]
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// Deserializes the per-plugin config block.
    pub fn config<T: DeserializeOwned>(&self) -> Result<T, PluginError> {
        serde_json::from_value(self.raw_config.clone())
            .map_err(|e| PluginError::InvalidConfig(e.to_string()))
    }

    #[must_use]
    pub fn core(&self) -> Arc<dyn CoreApi> {
        Arc::clone(&self.core_api)
    }

    /// Register an agent definition. Manifest must declare
    /// `Capability::Agent`.
    pub fn register_agent(&mut self, def: AgentDefinition) -> Result<(), PluginError> {
        self.assert_capability(&Capability::Agent, "Agent")?;
        self.registered_agents.push(def);
        Ok(())
    }

    /// Drain agents registered during `install`.
    #[must_use]
    pub fn take_registered_agents(&mut self) -> Vec<AgentDefinition> {
        std::mem::take(&mut self.registered_agents)
    }

    /// Register a custom tool. Manifest must declare `Capability::Tool`.
    pub fn register_tool(&mut self, tool: ToolHandle) -> Result<(), PluginError> {
        self.assert_capability(&Capability::Tool, "Tool")?;
        self.registered_tools.push(tool);
        Ok(())
    }

    /// Register a custom model provider. Manifest must declare
    /// `Capability::Provider`. Only one provider per plugin context;
    /// later registrations replace earlier ones with a logged warning.
    pub fn register_provider(
        &mut self,
        provider: Arc<dyn ModelProvider>,
    ) -> Result<(), PluginError> {
        self.assert_capability(&Capability::Provider, "Provider")?;
        if self.registered_provider.is_some() {
            tracing::warn!(
                plugin = %self.manifest.id,
                "register_provider called twice; later registration wins"
            );
        }
        self.registered_provider = Some(provider);
        Ok(())
    }

    /// Drain all registrations into a single struct the host consumes.
    #[must_use]
    pub fn into_registrations(self) -> PluginRegistrations {
        PluginRegistrations {
            agents: self.registered_agents,
            tools: self.registered_tools,
            provider: self.registered_provider,
            chains: HookChains {
                before_turn: self.before_turn,
                after_turn: self.after_turn,
                on_chat_params: self.on_chat_params,
                on_chat_messages: self.on_chat_messages,
                on_chat_headers: self.on_chat_headers,
                before_tool_call: self.before_tool_call,
                after_tool_call: self.after_tool_call,
                on_permission_ask: self.on_permission_ask,
                on_message: self.on_message,
                on_cost_tick: self.on_cost_tick,
                on_step_finish: self.on_step_finish,
                on_compaction: self.on_compaction,
                on_session_status: self.on_session_status,
                on_event: self.on_event,
            },
        }
    }

    fn assert_capability(
        &self,
        wanted: &Capability,
        label: &'static str,
    ) -> Result<(), PluginError> {
        if self.manifest.capabilities.contains(wanted) {
            Ok(())
        } else {
            Err(PluginError::Runtime(format!(
                "plugin {} cannot register {} — Capability::{} not declared in manifest",
                self.manifest.id, label, label
            )))
        }
    }

    fn assert_hook_capability(&self, kind: HookKind) -> Result<(), PluginError> {
        let declared = self
            .manifest
            .capabilities
            .iter()
            .any(|c| matches!(c, Capability::Hook(k) if *k == kind));
        if declared {
            Ok(())
        } else {
            Err(PluginError::Runtime(format!(
                "plugin {} cannot register hook {:?} — Capability::Hook(_) not declared in manifest",
                self.manifest.id, kind
            )))
        }
    }

    fn make_entry<I, F, Fut>(&mut self, kind: HookKind, priority: Priority, func: F) -> HookEntry<I>
    where
        F: Fn(I) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = HookResult<I>> + Send + 'static,
        I: Send + 'static,
    {
        let registration_index = self.next_index;
        self.next_index += 1;
        HookEntry {
            manifest_id: self.manifest.id.clone(),
            priority,
            registration_index,
            kind,
            func: Arc::new(move |input| Box::pin(func(input)) as HookFuture<I>),
        }
    }
}

/// Per-hook registration — generated for all 14 hook kinds via the
/// macro below. Each method signature is identical except for the
/// closure context type and the chain it pushes into.
macro_rules! impl_on_hook {
    ($method:ident, $field:ident, $kind:expr, $ctx:ty) => {
        impl PluginContext {
            pub fn $method<F, Fut>(
                &mut self,
                priority: Priority,
                func: F,
            ) -> Result<(), PluginError>
            where
                F: Fn($ctx) -> Fut + Send + Sync + 'static,
                Fut: Future<Output = HookResult<$ctx>> + Send + 'static,
            {
                self.assert_hook_capability($kind)?;
                let entry = self.make_entry($kind, priority, func);
                self.$field.push(entry);
                Ok(())
            }
        }
    };
}

impl_on_hook!(
    on_before_turn,
    before_turn,
    HookKind::BeforeTurn,
    BeforeTurnCtx
);
impl_on_hook!(on_after_turn, after_turn, HookKind::AfterTurn, AfterTurnCtx);
impl_on_hook!(
    on_chat_params,
    on_chat_params,
    HookKind::OnChatParams,
    OnChatParamsCtx
);
impl_on_hook!(
    on_chat_messages,
    on_chat_messages,
    HookKind::OnChatMessages,
    OnChatMessagesCtx
);
impl_on_hook!(
    on_chat_headers,
    on_chat_headers,
    HookKind::OnChatHeaders,
    OnChatHeadersCtx
);
impl_on_hook!(
    on_before_tool_call,
    before_tool_call,
    HookKind::BeforeToolCall,
    BeforeToolCallCtx
);
impl_on_hook!(
    on_after_tool_call,
    after_tool_call,
    HookKind::AfterToolCall,
    AfterToolCallCtx
);
impl_on_hook!(
    on_permission_ask,
    on_permission_ask,
    HookKind::OnPermissionAsk,
    OnPermissionAskCtx
);
impl_on_hook!(on_message, on_message, HookKind::OnMessage, OnMessageCtx);
impl_on_hook!(
    on_cost_tick,
    on_cost_tick,
    HookKind::OnCostTick,
    OnCostTickCtx
);
impl_on_hook!(
    on_step_finish,
    on_step_finish,
    HookKind::OnStepFinish,
    OnStepFinishCtx
);
impl_on_hook!(
    on_compaction,
    on_compaction,
    HookKind::OnCompaction,
    OnCompactionCtx
);
impl_on_hook!(
    on_session_status,
    on_session_status,
    HookKind::OnSessionStatus,
    OnSessionStatusCtx
);
impl_on_hook!(on_event, on_event, HookKind::OnEvent, OnEventCtx);

/// Typed back-channel into core. Plugins receive an `Arc<dyn CoreApi>`
/// inside `install` and may clone it into hook closures so they can
/// read session state, record cost, and emit events from inside any
/// dispatch site.
///
/// Read methods are async because the underlying memory store is.
/// `record_cost` is sync (DashMap update). `emit_event` is async
/// (matches the `EventSink::publish` shape).
#[async_trait]
pub trait CoreApi: Send + Sync + 'static {
    /// Latest persisted [`SessionMeta`] for `session_id`. Returns `None`
    /// for unknown / soft-deleted sessions.
    async fn current_session_meta(
        &self,
        session_id: openlet_core::types::session::SessionId,
    ) -> Option<openlet_core::types::session::SessionMeta>;

    /// Cumulative cost recorded across turns of `session_id`. Zero for
    /// unknown sessions.
    fn session_cost(
        &self,
        session_id: openlet_core::types::session::SessionId,
    ) -> rust_decimal::Decimal;

    /// Additively records cost (e.g. an integrator's billing-source
    /// plugin importing costs from a non-`OpenAiCompatProvider` model).
    /// Core's own cost calc still runs unchanged.
    fn record_cost(
        &self,
        session_id: openlet_core::types::session::SessionId,
        delta: rust_decimal::Decimal,
    );

    /// Bus passthrough so plugins can fan out custom events alongside
    /// core-emitted ones. Use `Persistence::Durable` for events that
    /// must survive a restart, `Persistence::Ephemeral` for hot-path
    /// telemetry.
    async fn emit_event(
        &self,
        event: openlet_core::types::event::AgentEvent,
        persistence: openlet_core::adapters::event_sink::Persistence,
    );

    /// Typed read of a single config key. Errors are stringified so
    /// plugin authors don't need to depend on the host's config crate.
    fn read_config(&self, key: &str) -> Result<serde_json::Value, String>;

    /// Trips the per-session cancellation token and marks the session
    /// `Cancelling`. The active turn (if any) unwinds; the next loop
    /// iteration short-circuits with `CoreError::Cancelled`. Idempotent
    /// — cancelling a finished or absent session is a no-op.
    ///
    /// `reason` is recorded on the emitted `SessionStatus` event so
    /// integrators (e.g. quota plugins) can attribute the abort.
    async fn cancel_session(
        &self,
        session_id: openlet_core::types::session::SessionId,
        reason: String,
    );
}
