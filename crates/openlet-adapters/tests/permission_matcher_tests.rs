//! Permission matcher matrix — last-match-wins, layered actions.

use std::sync::Arc;

use openlet_adapters::config_perm::ConfigPermissionMgr;
use openlet_core::adapters::permission_manager::PermissionManager;
use openlet_core::dispatch::{HookChains, HookEntry};
use openlet_core::hooks::{HookKind, HookResult, Priority, io::OnPermissionAskCtx};
use openlet_core::types::permission::{
    Decision, PermissionAction, PermissionCtx, PermissionMode, PermissionRequest, PermissionRule,
};
use openlet_core::types::session::SessionId;

fn ctx_mode(mode: PermissionMode) -> PermissionCtx {
    PermissionCtx {
        session_id: SessionId::new(),
        mode,
    }
}

fn req(p: &str) -> PermissionRequest {
    PermissionRequest {
        permission: p.to_string(),
        reason: None,
        timeout: None,
    }
}

fn rule(p: &str, a: PermissionAction) -> PermissionRule {
    PermissionRule {
        permission: p.to_string(),
        action: a,
    }
}

#[tokio::test]
async fn last_match_overrides_prior_deny() {
    let mgr = ConfigPermissionMgr::with_rules(vec![
        rule("read:**", PermissionAction::Deny),
        rule("read:*.md", PermissionAction::Allow),
    ])
    .unwrap();
    let d = mgr
        .check(
            ctx_mode(PermissionMode::WorkspaceWrite),
            req("read:NOTES.md"),
        )
        .await
        .unwrap();
    assert!(matches!(d, Decision::Allow));
}

#[tokio::test]
async fn last_match_wins_when_only_deny_at_end() {
    let mgr = ConfigPermissionMgr::with_rules(vec![
        rule("bash:**", PermissionAction::Allow),
        rule("bash:rm*", PermissionAction::Deny),
    ])
    .unwrap();
    let d = mgr
        .check(
            ctx_mode(PermissionMode::WorkspaceWrite),
            req("bash:rm -rf /"),
        )
        .await
        .unwrap();
    assert!(matches!(d, Decision::Deny { .. }));
}

#[tokio::test]
async fn no_rule_falls_through_to_mode_default() {
    let mgr = ConfigPermissionMgr::new();
    // WorkspaceWrite default = Ask.
    let d = mgr
        .check(ctx_mode(PermissionMode::WorkspaceWrite), req("read:foo"))
        .await
        .unwrap();
    assert!(matches!(d, Decision::Pending { .. }));
    // Danger default = Allow.
    let d = mgr
        .check(ctx_mode(PermissionMode::Danger), req("read:foo"))
        .await
        .unwrap();
    assert!(matches!(d, Decision::Allow));
}

#[tokio::test]
async fn ask_rule_creates_pending_entry() {
    let mgr =
        ConfigPermissionMgr::with_rules(vec![rule("edit:**", PermissionAction::Ask)]).unwrap();
    let d = mgr
        .check(ctx_mode(PermissionMode::WorkspaceWrite), req("edit:foo.md"))
        .await
        .unwrap();
    let ask_id = match d {
        Decision::Pending { ask_id } => ask_id,
        other => panic!("expected pending, got {other:?}"),
    };
    let deferred = mgr.take_deferred(ask_id).expect("ask still pending");
    mgr.reply(ask_id, Decision::Allow).await.unwrap();
    let resolved = deferred.await;
    assert!(matches!(resolved, Decision::Allow));
}

#[tokio::test]
async fn cancel_resolves_with_deny() {
    let mgr =
        ConfigPermissionMgr::with_rules(vec![rule("edit:**", PermissionAction::Ask)]).unwrap();
    let Decision::Pending { ask_id } = mgr
        .check(ctx_mode(PermissionMode::WorkspaceWrite), req("edit:foo"))
        .await
        .unwrap()
    else {
        panic!()
    };
    let deferred = mgr.take_deferred(ask_id).unwrap();
    mgr.cancel_ask(ask_id).await.unwrap();
    match deferred.await {
        Decision::Deny { feedback } => {
            assert_eq!(feedback.as_deref(), Some("ask cancelled"));
        }
        other => panic!("expected deny, got {other:?}"),
    }
}

#[tokio::test]
async fn record_always_appends_runtime_rule() {
    let mgr = ConfigPermissionMgr::new();
    mgr.record_always(
        openlet_core::types::permission::AlwaysScope::Session {
            id: SessionId::new(),
        },
        rule("read:*.md", PermissionAction::Allow),
    )
    .await
    .unwrap();
    let d = mgr
        .check(
            ctx_mode(PermissionMode::WorkspaceWrite),
            req("read:NOTES.md"),
        )
        .await
        .unwrap();
    assert!(matches!(d, Decision::Allow));
}

fn permission_entry<F, Fut>(manifest_id: &str, f: F) -> HookEntry<OnPermissionAskCtx>
where
    F: Fn(OnPermissionAskCtx) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = HookResult<OnPermissionAskCtx>> + Send + 'static,
{
    HookEntry {
        manifest_id: manifest_id.to_string(),
        priority: Priority(50),
        registration_index: 0,
        kind: HookKind::OnPermissionAsk,
        func: Arc::new(move |c| Box::pin(f(c))),
    }
}

#[tokio::test]
async fn on_permission_ask_replace_overrides_ruleset() {
    // Plugin overrides the decision before the ruleset runs. Even with
    // a Deny rule installed, the Replace decision wins.
    let mut chains = HookChains::new();
    chains
        .on_permission_ask
        .push(permission_entry("policy", |mut c| async move {
            c.decision = Some(Decision::Allow);
            HookResult::Replace(c)
        }));

    let mgr = ConfigPermissionMgr::with_rules(vec![rule("read:**", PermissionAction::Deny)])
        .unwrap()
        .with_hook_chains(Arc::new(chains));

    let d = mgr
        .check(ctx_mode(PermissionMode::WorkspaceWrite), req("read:secret"))
        .await
        .unwrap();
    assert!(matches!(d, Decision::Allow));
}

#[tokio::test]
async fn on_permission_ask_deny_short_circuits_ruleset() {
    // Deny outcome short-circuits before ruleset evaluation; even an
    // explicit Allow rule is bypassed.
    let mut chains = HookChains::new();
    chains
        .on_permission_ask
        .push(permission_entry("policy", |_c| async move {
            HookResult::Deny {
                reason: "blocked by plugin".to_string(),
                feedback: Some("policy violation".to_string()),
            }
        }));

    let mgr = ConfigPermissionMgr::with_rules(vec![rule("read:**", PermissionAction::Allow)])
        .unwrap()
        .with_hook_chains(Arc::new(chains));

    let d = mgr
        .check(ctx_mode(PermissionMode::WorkspaceWrite), req("read:foo"))
        .await
        .unwrap();
    match d {
        Decision::Deny { feedback } => {
            assert_eq!(feedback.as_deref(), Some("policy violation"));
        }
        other => panic!("expected Deny from plugin, got {other:?}"),
    }
}

#[tokio::test]
async fn on_permission_ask_continue_falls_through_to_ruleset() {
    // Continue without setting decision threads back to the ruleset,
    // which then applies its own Deny.
    let mut chains = HookChains::new();
    chains
        .on_permission_ask
        .push(permission_entry("noop", |c| async move {
            HookResult::Continue(c)
        }));

    let mgr = ConfigPermissionMgr::with_rules(vec![rule("read:**", PermissionAction::Deny)])
        .unwrap()
        .with_hook_chains(Arc::new(chains));

    let d = mgr
        .check(ctx_mode(PermissionMode::WorkspaceWrite), req("read:foo"))
        .await
        .unwrap();
    assert!(matches!(d, Decision::Deny { .. }));
}
