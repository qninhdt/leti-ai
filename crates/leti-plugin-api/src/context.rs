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
use leti_core::adapters::model_provider::ModelProvider;
use leti_core::agent::AgentDefinition;
use leti_core::tools::ToolHandle;
use serde::de::DeserializeOwned;

use crate::dispatch::{HookChains, HookEntry, HookFuture};
use crate::hooks::{
    HookKind, HookResult, Priority,
    io::{
        AfterToolCallCtx, AfterTurnCtx, BeforeToolCallCtx, BeforeTurnCtx, NotificationCtx,
        NotificationLevel, OnChatHeadersCtx, OnChatMessagesCtx, OnChatParamsCtx, OnCompactionCtx,
        OnCostTickCtx, OnEventCtx, OnMessageCtx, OnPermissionAskCtx, OnSessionStatusCtx,
        OnStepFinishCtx,
    },
};
use crate::manifest::{Capability, PluginManifest};
use crate::plugin::PluginError;

/// The ONE canonical hook-kind list. Every per-kind site — the
/// [`PluginContext`] chain fields, their `new()` initializers, the
/// `into_registrations` mapping, and the `on_*` registration methods — is
/// generated from this single list by [`hook_registration_sites!`]. Adding a
/// hook kind is now a single-line edit here (plus the `HookKind` variant,
/// `HookChains` field, and `io` ctx struct in leti-core).
///
/// Tuple shape: `(method_name, chain_field, HookKind variant, CtxType)`.
macro_rules! for_each_hook_kind {
    ($macro:ident) => {
        $macro! {
            (on_before_turn,     before_turn,       BeforeTurn,      BeforeTurnCtx),
            (on_after_turn,      after_turn,        AfterTurn,       AfterTurnCtx),
            (on_chat_params,     on_chat_params,    OnChatParams,    OnChatParamsCtx),
            (on_chat_messages,   on_chat_messages,  OnChatMessages,  OnChatMessagesCtx),
            (on_chat_headers,    on_chat_headers,   OnChatHeaders,   OnChatHeadersCtx),
            (on_before_tool_call, before_tool_call, BeforeToolCall,  BeforeToolCallCtx),
            (on_after_tool_call, after_tool_call,   AfterToolCall,   AfterToolCallCtx),
            (on_permission_ask,  on_permission_ask, OnPermissionAsk, OnPermissionAskCtx),
            (on_message,         on_message,        OnMessage,       OnMessageCtx),
            (on_cost_tick,       on_cost_tick,      OnCostTick,      OnCostTickCtx),
            (on_step_finish,     on_step_finish,    OnStepFinish,    OnStepFinishCtx),
            (on_compaction,      on_compaction,     OnCompaction,    OnCompactionCtx),
            (on_session_status,  on_session_status, OnSessionStatus, OnSessionStatusCtx),
            (on_event,           on_event,          OnEvent,         OnEventCtx),
            (on_notification,    notification,      Notification,    NotificationCtx),
        }
    };
}

/// Generate the `PluginContext` struct with its fixed fields plus one
/// `Vec<HookEntry<Ctx>>` chain field per hook kind.
macro_rules! define_plugin_context {
    ($( ($method:ident, $field:ident, $kind:ident, $ctx:ty) ),+ $(,)?) => {
        /// Registration API exposed to plugins during `install`.
        pub struct PluginContext {
            manifest: PluginManifest,
            raw_config: serde_json::Value,
            core_api: Arc<dyn CoreApi>,
            registered_agents: Vec<AgentDefinition>,
            registered_tools: Vec<ToolHandle>,
            registered_provider: Option<Arc<dyn ModelProvider>>,
            next_index: usize,
            $( pub(crate) $field: Vec<HookEntry<$ctx>>, )+
        }
    };
}
for_each_hook_kind!(define_plugin_context);

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
        macro_rules! init_context {
            ($( ($method:ident, $field:ident, $kind:ident, $ctx:ty) ),+ $(,)?) => {
                Self {
                    manifest,
                    raw_config,
                    core_api,
                    registered_agents: Vec::new(),
                    registered_tools: Vec::new(),
                    registered_provider: None,
                    next_index: 0,
                    $( $field: Vec::new(), )+
                }
            };
        }
        for_each_hook_kind!(init_context)
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
        macro_rules! drain_chains {
            ($( ($method:ident, $field:ident, $kind:ident, $ctx:ty) ),+ $(,)?) => {
                HookChains { $( $field: self.$field, )+ }
            };
        }
        PluginRegistrations {
            agents: self.registered_agents,
            tools: self.registered_tools,
            provider: self.registered_provider,
            chains: for_each_hook_kind!(drain_chains),
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

/// Per-hook registration — generates one `on_*` method per hook kind
/// from the canonical [`for_each_hook_kind!`] list. Each method signature
/// is identical except for the closure context type and the chain it
/// pushes into.
///
/// Adding a new hook kind is now a single-line edit to `for_each_hook_kind!`
/// (which drives the struct fields, `new()`, `into_registrations`, AND these
/// methods) plus its `HookKind` variant + `HookChains` field + `io` ctx
/// struct in leti-core.
macro_rules! impl_on_hook {
    ($( ($method:ident, $field:ident, $kind:ident, $ctx:ty) ),+ $(,)?) => {
        impl PluginContext {
            $(
                pub fn $method<F, Fut>(
                    &mut self,
                    priority: Priority,
                    func: F,
                ) -> Result<(), PluginError>
                where
                    F: Fn($ctx) -> Fut + Send + Sync + 'static,
                    Fut: Future<Output = HookResult<$ctx>> + Send + 'static,
                {
                    self.assert_hook_capability(HookKind::$kind)?;
                    let entry = self.make_entry(HookKind::$kind, priority, func);
                    self.$field.push(entry);
                    Ok(())
                }
            )+
        }
    };
}
for_each_hook_kind!(impl_on_hook);

impl PluginContext {
    /// Emit a user-facing notification. Pushes a [`NotificationCtx`]
    /// through the [`CoreApi::emit_notification`] back-channel which the
    /// host runs through both the notification hook chain and the SSE
    /// `notification.emitted` event.
    ///
    /// `body` is redacted by the host before SSE emission. Per-session
    /// rate limiting (10/sec cumulative across plugins) caps misbehaving
    /// plugins from flooding the channel — overflow drops the
    /// notification and emits a `tracing::warn!` so cloud operators can
    /// see the offender without parsing logs.
    pub async fn emit_notification(
        &self,
        session_id: Option<leti_core::types::session::SessionId>,
        level: NotificationLevel,
        title: impl Into<String>,
        body: impl Into<String>,
    ) {
        self.core_api
            .emit_notification(
                session_id,
                level,
                title.into(),
                body.into(),
                self.manifest.id.clone(),
            )
            .await;
    }
}

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
        session_id: leti_core::types::session::SessionId,
    ) -> Option<leti_core::types::session::SessionMeta>;

    /// Cumulative cost recorded across turns of `session_id`. Zero for
    /// unknown sessions.
    fn session_cost(
        &self,
        session_id: leti_core::types::session::SessionId,
    ) -> rust_decimal::Decimal;

    /// Additively records cost (e.g. an integrator's billing-source
    /// plugin importing costs from a non-`OpenAiCompatProvider` model).
    /// Core's own cost calc still runs unchanged.
    fn record_cost(
        &self,
        session_id: leti_core::types::session::SessionId,
        delta: rust_decimal::Decimal,
    );

    /// Bus passthrough so plugins can fan out custom events alongside
    /// core-emitted ones. Use `Persistence::Durable` for events that
    /// must survive a restart, `Persistence::Ephemeral` for hot-path
    /// telemetry.
    async fn emit_event(
        &self,
        event: leti_core::types::event::AgentEvent,
        persistence: leti_core::adapters::event_sink::Persistence,
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
        session_id: leti_core::types::session::SessionId,
        reason: String,
    );

    /// Plugin-emitted user-facing notification fan-out. Implementations
    /// MUST: (1) run the [`HookKind::Notification`] chain so observer
    /// plugins see it, (2) apply the secret redactor to `body`,
    /// (3) enforce a per-session cumulative rate limit (10/sec) and drop
    /// surplus emits with a tracing warn, (4) publish
    /// `AgentEvent::NotificationEmitted` durably for SSE replay.
    ///
    /// `plugin_id` is set by `PluginContext::emit_notification` from
    /// the manifest — plugins cannot spoof this field.
    async fn emit_notification(
        &self,
        session_id: Option<leti_core::types::session::SessionId>,
        level: NotificationLevel,
        title: String,
        body: String,
        plugin_id: String,
    );
}
