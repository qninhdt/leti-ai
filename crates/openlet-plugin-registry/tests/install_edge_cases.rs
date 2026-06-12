//! Edge-case coverage for [`install_all`] — the boot-time plugin merge.
//!
//! `install_all` is the single choke point where every plugin's
//! registrations are drained, validated, and merged. A bug here corrupts
//! server boot for ALL plugins, so these tests pin its failure modes:
//!   - duplicate plugin id is rejected,
//!   - incompatible `core_version_req` is rejected,
//!   - a panicking `install` is caught (not a crash) and surfaces the message,
//!   - two plugins claiming the same tool id collide,
//!   - two providers → first wins (non-fatal),
//!   - a clean two-plugin install merges every hook chain.
//!
//! Each test drives REAL `Plugin` trait impls through `install_all`; nothing
//! is mocked away — the configurable `TestPlugin` below registers actual
//! agents/tools/providers/hooks via the public `PluginContext` API.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use openlet_core::adapters::event_sink::Persistence;
use openlet_core::adapters::model_provider::{
    ChatRequest, ChatStream, ModelPricing, ModelProvider,
};
use openlet_core::adapters::tool_executor::ToolCtx;
use openlet_core::error::{ProviderError, ToolError};
use openlet_core::tools::{Tool, ToolHandle};
use openlet_core::types::event::AgentEvent;
use openlet_core::types::permission::PermissionRequest;
use openlet_core::types::session::{SessionId, SessionMeta};
use openlet_plugin_api::context::{CoreApi, PluginContext};
use openlet_plugin_api::hooks::io::NotificationLevel;
use openlet_plugin_api::hooks::{HookKind, HookResult, Priority};
use openlet_plugin_api::manifest::{Capability, PluginManifest};
use openlet_plugin_api::plugin::{Plugin, PluginError};
use openlet_plugin_registry::install_all;
use schemars::JsonSchema;
use semver::{Version, VersionReq};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

// --- Test doubles ----------------------------------------------------------

/// No-op `CoreApi` — `install_all` clones it into every `PluginContext`,
/// but none of these install paths call back into core.
struct NoopCoreApi;

#[async_trait]
impl CoreApi for NoopCoreApi {
    async fn current_session_meta(&self, _: SessionId) -> Option<SessionMeta> {
        None
    }
    fn session_cost(&self, _: SessionId) -> rust_decimal::Decimal {
        rust_decimal::Decimal::ZERO
    }
    fn record_cost(&self, _: SessionId, _: rust_decimal::Decimal) {}
    async fn emit_event(&self, _: AgentEvent, _: Persistence) {}
    fn read_config(&self, _: &str) -> Result<serde_json::Value, String> {
        Ok(serde_json::Value::Null)
    }
    async fn cancel_session(&self, _: SessionId, _: String) {}
    async fn emit_notification(
        &self,
        _: Option<SessionId>,
        _: NotificationLevel,
        _: String,
        _: String,
        _: String,
    ) {
    }
}

/// Minimal real tool whose wire name is configurable, so two plugins can be
/// made to claim the SAME tool id (the collision case).
struct NamedTool {
    name: &'static str,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct EmptyInput {}

#[async_trait]
impl Tool for NamedTool {
    type Input = EmptyInput;
    type Output = String;

    fn name(&self) -> &'static str {
        self.name
    }
    fn description(&self) -> &'static str {
        "test tool"
    }
    fn permission(&self, _: &Self::Input) -> PermissionRequest {
        PermissionRequest {
            permission: "test".to_string(),
            reason: None,
            timeout: None,
        }
    }
    async fn run(&self, _: ToolCtx, _: Self::Input) -> Result<Self::Output, ToolError> {
        Ok("ok".to_string())
    }
}

/// A stub provider — registered so we can prove the first-wins rule. Identity
/// is carried in `tag` so the test can assert WHICH provider survived.
struct StubProvider {
    tag: &'static str,
}

#[async_trait]
impl ModelProvider for StubProvider {
    async fn chat_stream(
        &self,
        _: ChatRequest,
        _: CancellationToken,
    ) -> Result<ChatStream, ProviderError> {
        Err(ProviderError::Network(format!(
            "stub provider {}",
            self.tag
        )))
    }
    fn pricing(&self, _: &str) -> Option<ModelPricing> {
        None
    }
}

/// Configurable plugin used to drive every edge case. Knobs select the
/// manifest id, version requirement, capabilities, and what it registers.
struct TestPlugin {
    manifest: PluginManifest,
    tool_name: Option<&'static str>,
    provider_tag: Option<&'static str>,
    register_before_turn: bool,
    register_on_event: bool,
    panic_in_install: Option<&'static str>,
}

impl TestPlugin {
    fn builder(id: &'static str) -> TestPluginBuilder {
        TestPluginBuilder {
            id,
            core_req: VersionReq::STAR,
            caps: Vec::new(),
            tool_name: None,
            provider_tag: None,
            register_before_turn: false,
            register_on_event: false,
            panic_in_install: None,
        }
    }
}

struct TestPluginBuilder {
    id: &'static str,
    core_req: VersionReq,
    caps: Vec<Capability>,
    tool_name: Option<&'static str>,
    provider_tag: Option<&'static str>,
    register_before_turn: bool,
    register_on_event: bool,
    panic_in_install: Option<&'static str>,
}

impl TestPluginBuilder {
    fn core_req(mut self, req: &str) -> Self {
        self.core_req = VersionReq::parse(req).expect("valid req");
        self
    }
    fn with_tool(mut self, name: &'static str) -> Self {
        self.tool_name = Some(name);
        self.caps.push(Capability::Tool);
        self
    }
    fn with_provider(mut self, tag: &'static str) -> Self {
        self.provider_tag = Some(tag);
        self.caps.push(Capability::Provider);
        self
    }
    fn with_before_turn(mut self) -> Self {
        self.register_before_turn = true;
        self.caps.push(Capability::Hook(HookKind::BeforeTurn));
        self
    }
    fn with_on_event(mut self) -> Self {
        self.register_on_event = true;
        self.caps.push(Capability::Hook(HookKind::OnEvent));
        self
    }
    fn panicking(mut self, msg: &'static str) -> Self {
        self.panic_in_install = Some(msg);
        self
    }
    fn build(self) -> TestPlugin {
        TestPlugin {
            manifest: PluginManifest::builder(self.id, self.id)
                .version(Version::new(0, 1, 0))
                .core_version_req(self.core_req)
                .capabilities(self.caps)
                .build(),
            tool_name: self.tool_name,
            provider_tag: self.provider_tag,
            register_before_turn: self.register_before_turn,
            register_on_event: self.register_on_event,
            panic_in_install: self.panic_in_install,
        }
    }
}

#[async_trait]
impl Plugin for TestPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    async fn install(&self, ctx: &mut PluginContext) -> Result<(), PluginError> {
        if let Some(msg) = self.panic_in_install {
            panic!("{msg}");
        }
        if let Some(name) = self.tool_name {
            let tool: ToolHandle = Arc::new(NamedTool { name });
            ctx.register_tool(tool)?;
        }
        if let Some(tag) = self.provider_tag {
            ctx.register_provider(Arc::new(StubProvider { tag }))?;
        }
        if self.register_before_turn {
            ctx.on_before_turn(Priority::default(), |x| async { HookResult::Continue(x) })?;
        }
        if self.register_on_event {
            ctx.on_event(Priority::default(), |x| async { HookResult::Continue(x) })?;
        }
        Ok(())
    }
}

fn core_api() -> Arc<dyn CoreApi> {
    Arc::new(NoopCoreApi)
}

fn no_configs() -> HashMap<String, serde_json::Value> {
    HashMap::new()
}

// --- Tests -----------------------------------------------------------------

#[tokio::test]
async fn duplicate_plugin_id_is_rejected() {
    // Two distinct plugin instances claiming the SAME manifest id. The
    // second must trip the `seen_ids` guard rather than silently shadow.
    let plugins: Vec<Arc<dyn Plugin>> = vec![
        Arc::new(TestPlugin::builder("dup.id").build()),
        Arc::new(TestPlugin::builder("dup.id").build()),
    ];

    let err = match install_all(plugins, &no_configs(), core_api()).await {
        Ok(_) => panic!("duplicate id must error"),
        Err(e) => e,
    };

    match err {
        PluginError::Runtime(msg) => {
            assert!(
                msg.contains("duplicate plugin id") && msg.contains("dup.id"),
                "error must name the duplicated id, got: {msg}"
            );
        }
        other => panic!("expected Runtime error, got {other:?}"),
    }
}

#[tokio::test]
async fn incompatible_core_version_is_rejected() {
    // `install_all` resolves `core` from CARGO_PKG_VERSION (0.x). A plugin
    // demanding `>=99.0.0` can never match → IncompatibleCoreVersion.
    let plugins: Vec<Arc<dyn Plugin>> = vec![Arc::new(
        TestPlugin::builder("too.new").core_req(">=99.0.0").build(),
    )];

    let err = match install_all(plugins, &no_configs(), core_api()).await {
        Ok(_) => panic!("incompatible core version must error"),
        Err(e) => e,
    };

    match err {
        PluginError::IncompatibleCoreVersion { id, req, .. } => {
            assert_eq!(id, "too.new");
            assert_eq!(req, ">=99.0.0");
        }
        other => panic!("expected IncompatibleCoreVersion, got {other:?}"),
    }
}

#[tokio::test]
async fn panicking_install_is_caught_and_surfaces_message() {
    // A buggy plugin that panics inside `install` must NOT unwind through
    // `install_all` (which would abort server boot). The panic is caught and
    // the message is threaded into a Runtime error.
    let plugins: Vec<Arc<dyn Plugin>> = vec![Arc::new(
        TestPlugin::builder("boom")
            .panicking("kaboom in install")
            .build(),
    )];

    let err = match install_all(plugins, &no_configs(), core_api()).await {
        Ok(_) => panic!("panicking install must surface an error, not crash"),
        Err(e) => e,
    };

    match err {
        PluginError::Runtime(msg) => {
            assert!(
                msg.contains("boom") && msg.contains("kaboom in install"),
                "error must carry both plugin id and panic message, got: {msg}"
            );
        }
        other => panic!("expected Runtime error, got {other:?}"),
    }
}

#[tokio::test]
async fn same_tool_id_across_plugins_collides() {
    // Two plugins each register a tool named `shared_tool`. The second
    // registration must be rejected — first-registration wins, but the loser
    // is a hard error (silent shadowing would mis-route dispatch).
    let plugins: Vec<Arc<dyn Plugin>> = vec![
        Arc::new(
            TestPlugin::builder("plug.a")
                .with_tool("shared_tool")
                .build(),
        ),
        Arc::new(
            TestPlugin::builder("plug.b")
                .with_tool("shared_tool")
                .build(),
        ),
    ];

    let err = match install_all(plugins, &no_configs(), core_api()).await {
        Ok(_) => panic!("colliding tool id must error"),
        Err(e) => e,
    };

    match err {
        PluginError::Runtime(msg) => {
            assert!(
                msg.contains("tool id collision") && msg.contains("shared_tool"),
                "error must name the colliding tool id, got: {msg}"
            );
        }
        other => panic!("expected Runtime error, got {other:?}"),
    }
}

#[tokio::test]
async fn two_providers_first_wins_non_fatal() {
    // Provider conflicts are NON-fatal: the first plugin to register a
    // provider wins, the later one is ignored with a logged warning. The
    // install must SUCCEED and the surviving provider must be the first one.
    let plugins: Vec<Arc<dyn Plugin>> = vec![
        Arc::new(
            TestPlugin::builder("prov.first")
                .with_provider("first")
                .build(),
        ),
        Arc::new(
            TestPlugin::builder("prov.second")
                .with_provider("second")
                .build(),
        ),
    ];

    let installed = install_all(plugins, &no_configs(), core_api())
        .await
        .expect("provider conflict is non-fatal — install must succeed");

    let provider = installed.provider.expect("a provider must survive");
    // Prove WHICH provider won by reading the identity it embeds in its
    // error message (the stub returns its tag on chat_stream).
    let err = match provider
        .chat_stream(
            ChatRequest {
                model: "x".to_string(),
                messages: Vec::new(),
                system: None,
                max_tokens: None,
                temperature: None,
                tools: Vec::new(),
                stream: true,
                headers: Default::default(),
            },
            CancellationToken::new(),
        )
        .await
    {
        Ok(_) => panic!("stub provider always errors"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("first"),
        "first-registered provider must win, got: {err}"
    );
}

#[tokio::test]
async fn clean_two_plugin_install_merges_hook_chains() {
    // Happy path: two plugins, each registering a different hook + tool. All
    // registrations must merge: 2 tools, 2 manifests, and both hook chains
    // populated (before_turn from one, on_event from the other).
    let plugins: Vec<Arc<dyn Plugin>> = vec![
        Arc::new(
            TestPlugin::builder("merge.a")
                .with_tool("tool_a")
                .with_before_turn()
                .build(),
        ),
        Arc::new(
            TestPlugin::builder("merge.b")
                .with_tool("tool_b")
                .with_on_event()
                .build(),
        ),
    ];

    let installed = install_all(plugins, &no_configs(), core_api())
        .await
        .expect("clean install must succeed");

    assert_eq!(installed.manifests.len(), 2, "both manifests recorded");
    assert_eq!(installed.tools.len(), 2, "both tools registered");
    let tool_names: Vec<&str> = installed.tools.iter().map(|t| t.name()).collect();
    assert!(tool_names.contains(&"tool_a"));
    assert!(tool_names.contains(&"tool_b"));

    assert_eq!(
        installed.chains.before_turn.len(),
        1,
        "before_turn chain carries merge.a's hook"
    );
    assert_eq!(
        installed.chains.on_event.len(),
        1,
        "on_event chain carries merge.b's hook"
    );
    assert!(installed.provider.is_none(), "no provider registered");
}
