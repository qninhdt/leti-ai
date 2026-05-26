//! Capability-gating tests for [`PluginContext`].
//!
//! Slice 3a's macro generates 14 `on_*` registration methods. Each
//! must reject if the manifest doesn't declare the matching
//! `Capability::Hook(_)`. Same for `register_tool` / `register_provider`
//! gated on `Capability::Tool` / `Capability::Provider`. These tests lock
//! the gate so the macro can't silently drop it on any expansion.

use std::sync::Arc;

use openlet_core::adapters::event_sink::Persistence;
use openlet_core::types::event::AgentEvent;
use openlet_core::types::session::{SessionId, SessionMeta};
use openlet_plugin_api::context::{CoreApi, PluginContext};
use openlet_plugin_api::hooks::{HookKind, HookResult, Priority};
use openlet_plugin_api::manifest::{Capability, PluginManifest};
use openlet_plugin_api::plugin::PluginError;
use semver::{Version, VersionReq};

struct NoopCoreApi;

#[async_trait::async_trait]
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
        _: openlet_core::hooks::io::NotificationLevel,
        _: String,
        _: String,
        _: String,
    ) {
    }
}

fn manifest_with(caps: Vec<Capability>) -> PluginManifest {
    PluginManifest {
        id: "test.plugin".to_string(),
        name: "Test Plugin".to_string(),
        version: Version::new(0, 1, 0),
        description: "test".to_string(),
        author: None,
        capabilities: caps,
        core_version_req: VersionReq::STAR,
        default_priority: 50,
        config_schema: None,
    }
}

fn ctx_with(caps: Vec<Capability>) -> PluginContext {
    PluginContext::new(
        manifest_with(caps),
        serde_json::Value::Null,
        Arc::new(NoopCoreApi),
    )
}

type RegisterFn = fn(&mut PluginContext) -> Result<(), PluginError>;

#[tokio::test]
async fn each_on_hook_method_rejects_undeclared_capability() {
    // Manifest declares NO Hook capabilities.
    let cases: Vec<(HookKind, RegisterFn)> = vec![
        (HookKind::BeforeTurn, |c| {
            c.on_before_turn(Priority::default(), |x| async { HookResult::Continue(x) })
        }),
        (HookKind::AfterTurn, |c| {
            c.on_after_turn(Priority::default(), |x| async { HookResult::Continue(x) })
        }),
        (HookKind::OnChatParams, |c| {
            c.on_chat_params(Priority::default(), |x| async { HookResult::Continue(x) })
        }),
        (HookKind::OnChatMessages, |c| {
            c.on_chat_messages(Priority::default(), |x| async { HookResult::Continue(x) })
        }),
        (HookKind::OnChatHeaders, |c| {
            c.on_chat_headers(Priority::default(), |x| async { HookResult::Continue(x) })
        }),
        (HookKind::BeforeToolCall, |c| {
            c.on_before_tool_call(Priority::default(), |x| async { HookResult::Continue(x) })
        }),
        (HookKind::AfterToolCall, |c| {
            c.on_after_tool_call(Priority::default(), |x| async { HookResult::Continue(x) })
        }),
        (HookKind::OnPermissionAsk, |c| {
            c.on_permission_ask(Priority::default(), |x| async { HookResult::Continue(x) })
        }),
        (HookKind::OnMessage, |c| {
            c.on_message(Priority::default(), |x| async { HookResult::Continue(x) })
        }),
        (HookKind::OnCostTick, |c| {
            c.on_cost_tick(Priority::default(), |x| async { HookResult::Continue(x) })
        }),
        (HookKind::OnStepFinish, |c| {
            c.on_step_finish(Priority::default(), |x| async { HookResult::Continue(x) })
        }),
        (HookKind::OnCompaction, |c| {
            c.on_compaction(Priority::default(), |x| async { HookResult::Continue(x) })
        }),
        (HookKind::OnSessionStatus, |c| {
            c.on_session_status(Priority::default(), |x| async { HookResult::Continue(x) })
        }),
        (HookKind::OnEvent, |c| {
            c.on_event(Priority::default(), |x| async { HookResult::Continue(x) })
        }),
    ];
    assert_eq!(cases.len(), 14, "all 14 hook kinds must be covered");

    for (kind, register) in cases {
        let mut ctx = ctx_with(vec![]);
        let result = register(&mut ctx);
        match result {
            Err(PluginError::Runtime(msg)) => {
                assert!(
                    msg.contains(&format!("{kind:?}")),
                    "{kind:?}: error must name the rejected hook kind, got: {msg}"
                );
            }
            Err(other) => panic!("{kind:?}: expected PluginError::Runtime, got {other:?}"),
            Ok(()) => panic!("{kind:?}: expected rejection without capability"),
        }
    }
}

#[tokio::test]
async fn declared_hook_capability_allows_registration() {
    let mut ctx = ctx_with(vec![Capability::Hook(HookKind::BeforeTurn)]);
    let r = ctx.on_before_turn(Priority::default(), |x| async { HookResult::Continue(x) });
    assert!(
        r.is_ok(),
        "registration with declared capability must succeed"
    );
}
