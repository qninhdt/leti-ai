//! `FailClosedAskManager` — the Phase 2 fail-closed-Ask shim applied to
//! autonomous (non-`User`) injected turns.
//!
//! Contract: a `Pending` (interactive-ask) decision from the inner
//! manager is rewritten to `Deny` (no human is attached to answer), and
//! the underlying ask is cancelled so nothing parks. `Allow` and `Deny`
//! pass through unchanged.

use std::sync::Arc;

use leti_core::adapters::permission_manager::PermissionManager;
use leti_core::runtime::injected_permission::FailClosedAskManager;
use leti_core::types::permission::{
    AskId, Decision, PermissionCtx, PermissionMode, PermissionRequest,
};
use leti_core::types::session::SessionId;

mod common;
use common::mock_permission::ScriptedPermission;

fn ctx() -> PermissionCtx {
    PermissionCtx {
        session_id: SessionId::new(),
        mode: PermissionMode::WorkspaceWrite,
        interaction_mode: Default::default(),
        ext: Default::default(),
    }
}

fn req(perm: &str) -> PermissionRequest {
    PermissionRequest {
        permission: perm.to_string(),
        reason: None,
        timeout: None,
    }
}

#[tokio::test]
async fn pending_decision_fails_closed_to_deny() {
    // Inner manager wants to prompt a human (`Pending`) — the shim must
    // convert it to `Deny` because an injected turn has no human answerer.
    let inner = Arc::new(ScriptedPermission::new([Decision::Pending {
        ask_id: AskId::new(),
    }]));
    let shim = FailClosedAskManager::new(inner);

    let decision = shim.check(ctx(), req("bash:rm -rf /")).await.unwrap();
    assert!(
        matches!(decision, Decision::Deny { .. }),
        "Pending must fail closed to Deny, got {decision:?}"
    );
}

#[tokio::test]
async fn allow_passes_through() {
    let inner = Arc::new(ScriptedPermission::new([Decision::Allow]));
    let shim = FailClosedAskManager::new(inner);

    let decision = shim.check(ctx(), req("read:foo.rs")).await.unwrap();
    assert!(
        matches!(decision, Decision::Allow),
        "Allow must pass through"
    );
}

#[tokio::test]
async fn deny_passes_through_with_feedback() {
    let inner = Arc::new(ScriptedPermission::new([Decision::Deny {
        feedback: Some("blocked by rule".into()),
    }]));
    let shim = FailClosedAskManager::new(inner);

    let decision = shim.check(ctx(), req("write:secret")).await.unwrap();
    match decision {
        Decision::Deny { feedback } => {
            assert_eq!(feedback.as_deref(), Some("blocked by rule"));
        }
        other => panic!("Deny must pass through unchanged, got {other:?}"),
    }
}
