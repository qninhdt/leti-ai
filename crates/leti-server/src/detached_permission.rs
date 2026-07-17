//! Permission wrapper for explicitly detached user turns.
//!
//! It converts ordinary `Ask` outcomes into the configured detached policy,
//! but leaves explicit ruleset Deny decisions untouched. Network egress and
//! destructive shell subjects remain fail-closed unless the host has an
//! explicit rule for them.

use std::sync::Arc;

use async_trait::async_trait;
use leti_core::adapters::permission_manager::PermissionManager;
use leti_core::error::PermissionError;
use leti_core::permission::Deferred;
use leti_core::types::permission::{
    AlwaysScope, AskId, Decision, PermissionAction, PermissionCtx, PermissionRequest,
    PermissionRule,
};
use leti_core::types::session::DetachedAsk;

pub(crate) struct DetachedPermissionManager {
    inner: Arc<dyn PermissionManager>,
    on_ask: DetachedAsk,
}

impl DetachedPermissionManager {
    pub(crate) fn new(inner: Arc<dyn PermissionManager>, on_ask: DetachedAsk) -> Self {
        Self { inner, on_ask }
    }
}

fn hardened_ask(permission: &str) -> bool {
    let lower = permission.to_ascii_lowercase();
    if lower.starts_with("web_fetch:") {
        return true;
    }
    if !lower.starts_with("bash:") {
        return false;
    }
    [
        "rm -r",
        "rm -f",
        "rmdir ",
        "sudo ",
        "mkfs",
        " dd ",
        "shutdown",
        "reboot",
        "chmod -r",
        "chown -r",
        "git push -f",
        "--force",
        "--delete",
        "--mirror",
        "reset --hard",
        "| sh",
        "|sh",
        "| bash",
        "|bash",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

#[async_trait]
impl PermissionManager for DetachedPermissionManager {
    async fn check(
        &self,
        ctx: PermissionCtx,
        req: PermissionRequest,
    ) -> Result<Decision, PermissionError> {
        match self.inner.check(ctx, req.clone()).await? {
            Decision::Pending { ask_id } => {
                let _ = self.inner.cancel_ask(ask_id).await;
                if hardened_ask(&req.permission) {
                    Ok(Decision::Deny {
                        feedback: Some(
                            "detached mode requires an explicit allow for this operation".into(),
                        ),
                    })
                } else {
                    Ok(match self.on_ask {
                        DetachedAsk::Allow => Decision::Allow,
                        DetachedAsk::Deny => Decision::Deny {
                            feedback: Some("detached mode auto-denied Ask".into()),
                        },
                    })
                }
            }
            other => Ok(other),
        }
    }

    async fn reply(&self, ask_id: AskId, decision: Decision) -> Result<(), PermissionError> {
        self.inner.reply(ask_id, decision).await
    }

    async fn cancel_ask(&self, ask_id: AskId) -> Result<(), PermissionError> {
        self.inner.cancel_ask(ask_id).await
    }

    async fn record_always(
        &self,
        scope: AlwaysScope,
        rule: PermissionRule,
    ) -> Result<(), PermissionError> {
        self.inner.record_always(scope, rule).await
    }

    fn take_deferred(&self, ask_id: AskId) -> Option<Deferred<Decision>> {
        self.inner.take_deferred(ask_id)
    }

    fn peek_session_id(&self, ask_id: AskId) -> Option<leti_core::types::session::SessionId> {
        self.inner.peek_session_id(ask_id)
    }

    async fn accept_ask(
        &self,
        ask_id: AskId,
        scope: AlwaysScope,
        action: PermissionAction,
    ) -> Result<(), PermissionError> {
        self.inner.accept_ask(ask_id, scope, action).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leti_adapters::config_perm::ConfigPermissionMgr;
    use leti_core::types::permission::PermissionMode;
    use leti_core::types::session::InteractionMode;
    use leti_core::types::session::SessionId;

    fn ctx() -> PermissionCtx {
        PermissionCtx {
            session_id: SessionId::new(),
            mode: PermissionMode::WorkspaceWrite,
            interaction_mode: InteractionMode::Detached {
                on_ask: DetachedAsk::Allow,
            },
            ext: Default::default(),
        }
    }

    fn request(permission: &str) -> PermissionRequest {
        PermissionRequest::simple(permission)
    }

    #[tokio::test]
    async fn ordinary_ask_is_resolved_without_a_pending_wait() {
        let inner = Arc::new(
            ConfigPermissionMgr::with_rules(vec![PermissionRule {
                permission: "ordinary".into(),
                action: PermissionAction::Ask,
            }])
            .unwrap(),
        );
        let manager = DetachedPermissionManager::new(inner, DetachedAsk::Allow);
        assert!(matches!(
            manager.check(ctx(), request("ordinary")).await.unwrap(),
            Decision::Allow
        ));
    }

    #[tokio::test]
    async fn deny_policy_and_hardened_subjects_remain_denied() {
        let inner = Arc::new(
            ConfigPermissionMgr::with_rules(vec![PermissionRule {
                permission: "ordinary".into(),
                action: PermissionAction::Ask,
            }])
            .unwrap(),
        );
        let manager = DetachedPermissionManager::new(inner, DetachedAsk::Deny);
        assert!(matches!(
            manager.check(ctx(), request("ordinary")).await.unwrap(),
            Decision::Deny { .. }
        ));
        assert!(matches!(
            manager
                .check(ctx(), request("web_fetch:https://example.com"))
                .await
                .unwrap(),
            Decision::Deny { .. }
        ));
        assert!(matches!(
            manager
                .check(ctx(), request("bash:rm -rf /tmp/x"))
                .await
                .unwrap(),
            Decision::Deny { .. }
        ));
    }

    #[tokio::test]
    async fn explicit_deny_from_ruleset_is_not_overridden() {
        let inner = Arc::new(
            ConfigPermissionMgr::with_rules(vec![PermissionRule {
                permission: "ordinary".into(),
                action: PermissionAction::Deny,
            }])
            .unwrap(),
        );
        let manager = DetachedPermissionManager::new(inner, DetachedAsk::Allow);
        assert!(matches!(
            manager.check(ctx(), request("ordinary")).await.unwrap(),
            Decision::Deny { .. }
        ));
    }
}
