//! Manager — owns the compiled ruleset, the pending-ask map, and the
//! always-allow persistence bridge.

use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use openlet_core::adapters::permission_manager::PermissionManager;
use openlet_core::dispatch::{DispatchOutcome, HookChains, dispatch};
use openlet_core::error::PermissionError;
use openlet_core::hooks::io::OnPermissionAskCtx;
use openlet_core::permission::{Deferred, DeferredSender, deferred_pair};
use openlet_core::types::permission::{
    AlwaysScope, AskId, Decision, PermissionAction, PermissionCtx, PermissionMode,
    PermissionRequest, PermissionRule,
};
use tokio::sync::RwLock;

use super::ruleset::{CompiledRule, CompiledRuleset};

/// Per-pending-ask state. We carry the request alongside the sender so
/// the API layer can render a user-friendly prompt in the SSE event.
/// `deferred` is held until the runtime calls `take_deferred(ask_id)`,
/// at which point the runtime owns the receiver half of the oneshot.
pub struct PendingAsk {
    #[allow(dead_code)] // surfaced via SSE in phase 5
    pub request: PermissionRequest,
    pub sender: DeferredSender<Decision>,
    pub deferred: Option<Deferred<Decision>>,
}

/// Mode-default policy: in `ReadOnly` and `WorkspaceWrite` we ask if no
/// rule matches; in `Danger` we allow. Mirrors the claw-code mode table
/// (`permission_enforcer.rs`) but without their first-match shortcut.
fn fallback_for_mode(mode: PermissionMode) -> PermissionAction {
    match mode {
        PermissionMode::ReadOnly | PermissionMode::WorkspaceWrite => PermissionAction::Ask,
        PermissionMode::Danger => PermissionAction::Allow,
    }
}

#[derive(Default)]
pub struct ConfigPermissionMgr {
    inner: Arc<RwLock<CompiledRuleset>>,
    pending: Arc<DashMap<AskId, PendingAsk>>,
    hook_chains: Option<Arc<HookChains>>,
}

impl ConfigPermissionMgr {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach plugin hook chains so `on_permission_ask` runs before the
    /// configured ruleset. `Replace` overrides the decision; `Continue`
    /// falls through to the ruleset; `Deny` short-circuits the request.
    #[must_use]
    pub fn with_hook_chains(mut self, hook_chains: Arc<HookChains>) -> Self {
        self.hook_chains = Some(hook_chains);
        self
    }

    /// Construct from raw rules. Errors propagate from glob compilation.
    pub fn with_rules(rules: Vec<PermissionRule>) -> Result<Self, PermissionError> {
        let compiled =
            CompiledRuleset::from_rules(rules).map_err(|e| PermissionError::Io(e.to_string()))?;
        Ok(Self {
            inner: Arc::new(RwLock::new(compiled)),
            pending: Arc::new(DashMap::new()),
            hook_chains: None,
        })
    }

    /// Snapshot of pending asks — useful for the HTTP route that lists
    /// open prompts for a session.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Surrender the receiver half of an outstanding ask. Phase 4C: the
    /// runtime calls this immediately after `check()` returns
    /// `Decision::Pending`, then `.await`s the deferred. Returns `None`
    /// if the ask was already taken or never existed.
    pub fn take_deferred(&self, ask_id: AskId) -> Option<Deferred<Decision>> {
        self.pending.get_mut(&ask_id)?.deferred.take()
    }
}

#[async_trait]
impl PermissionManager for ConfigPermissionMgr {
    async fn check(
        &self,
        ctx: PermissionCtx,
        req: PermissionRequest,
    ) -> Result<Decision, PermissionError> {
        // OnPermissionAsk hook chain runs BEFORE the ruleset. Replace
        // overrides the decision; Continue falls through to the ruleset.
        let req = if let Some(chains) = self.hook_chains.as_ref() {
            let hook_ctx = OnPermissionAskCtx {
                request: Some(req.clone()),
                decision: None,
            };
            match dispatch(&chains.on_permission_ask, hook_ctx).await {
                DispatchOutcome::Completed(c) => {
                    if let Some(decision) = c.decision {
                        return Ok(decision);
                    }
                    c.request.unwrap_or(req)
                }
                DispatchOutcome::Stopped(c) => {
                    if let Some(decision) = c.decision {
                        return Ok(decision);
                    }
                    c.request.unwrap_or(req)
                }
                DispatchOutcome::Denied { feedback, .. } => {
                    return Ok(Decision::Deny { feedback });
                }
            }
        } else {
            req
        };

        let action = {
            let g = self.inner.read().await;
            g.evaluate(&req.permission)
                .map(|r| r.action)
                .unwrap_or_else(|| fallback_for_mode(ctx.mode))
        };

        match action {
            PermissionAction::Allow => Ok(Decision::Allow),
            PermissionAction::Deny => Ok(Decision::Deny {
                feedback: Some(format!(
                    "Permission denied by ruleset for {:?}",
                    req.permission
                )),
            }),
            PermissionAction::Ask => {
                let ask_id = AskId::new();
                let (deferred, sender) = deferred_pair(Decision::Deny {
                    feedback: Some("ask cancelled".into()),
                });
                self.pending.insert(
                    ask_id,
                    PendingAsk {
                        request: req,
                        sender,
                        deferred: Some(deferred),
                    },
                );
                Ok(Decision::Pending { ask_id })
            }
        }
    }

    async fn reply(&self, ask_id: AskId, decision: Decision) -> Result<(), PermissionError> {
        let (_, ask) = self
            .pending
            .remove(&ask_id)
            .ok_or(PermissionError::AskNotFound)?;
        // Drop on send-failure is fine — runtime already moved on.
        let _ = ask.sender.send(decision);
        Ok(())
    }

    async fn cancel_ask(&self, ask_id: AskId) -> Result<(), PermissionError> {
        let (_, ask) = self
            .pending
            .remove(&ask_id)
            .ok_or(PermissionError::AskNotFound)?;
        let _ = ask.sender.send(Decision::Deny {
            feedback: Some("ask cancelled".into()),
        });
        Ok(())
    }

    async fn record_always(
        &self,
        _scope: AlwaysScope,
        rule: PermissionRule,
    ) -> Result<(), PermissionError> {
        let compiled =
            CompiledRule::from_rule(rule).map_err(|e| PermissionError::Io(e.to_string()))?;
        let mut g = self.inner.write().await;
        g.push(compiled);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openlet_core::types::session::SessionId;

    fn ctx() -> PermissionCtx {
        PermissionCtx {
            session_id: SessionId::new(),
            mode: PermissionMode::WorkspaceWrite,
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
    async fn last_match_wins_allow_after_deny() {
        let rules = vec![
            PermissionRule {
                permission: "read:**".into(),
                action: PermissionAction::Deny,
            },
            PermissionRule {
                permission: "read:*.md".into(),
                action: PermissionAction::Allow,
            },
        ];
        let m = ConfigPermissionMgr::with_rules(rules).unwrap();
        let d = m.check(ctx(), req("read:NOTES.md")).await.unwrap();
        assert!(matches!(d, Decision::Allow));
    }

    #[tokio::test]
    async fn deny_when_last_match_is_deny() {
        let rules = vec![
            PermissionRule {
                permission: "bash:**".into(),
                action: PermissionAction::Allow,
            },
            PermissionRule {
                permission: "bash:rm*".into(),
                action: PermissionAction::Deny,
            },
        ];
        let m = ConfigPermissionMgr::with_rules(rules).unwrap();
        let d = m.check(ctx(), req("bash:rm -rf /")).await.unwrap();
        assert!(matches!(d, Decision::Deny { .. }));
    }

    #[tokio::test]
    async fn fallback_ask_when_no_rule() {
        let m = ConfigPermissionMgr::new();
        let d = m.check(ctx(), req("read:foo")).await.unwrap();
        assert!(matches!(d, Decision::Pending { .. }));
        assert_eq!(m.pending_count(), 1);
    }

    #[tokio::test]
    async fn danger_mode_allows_unmatched() {
        let m = ConfigPermissionMgr::new();
        let mut c = ctx();
        c.mode = PermissionMode::Danger;
        let d = m.check(c, req("bash:foo")).await.unwrap();
        assert!(matches!(d, Decision::Allow));
    }

    #[tokio::test]
    async fn record_always_appends_rule() {
        let m = ConfigPermissionMgr::new();
        m.record_always(
            AlwaysScope::Session {
                id: SessionId::new(),
            },
            PermissionRule {
                permission: "edit:*.md".into(),
                action: PermissionAction::Allow,
            },
        )
        .await
        .unwrap();
        let d = m.check(ctx(), req("edit:notes.md")).await.unwrap();
        assert!(matches!(d, Decision::Allow));
    }

    #[tokio::test]
    async fn reply_unknown_ask_errors() {
        let m = ConfigPermissionMgr::new();
        let res = m.reply(AskId::new(), Decision::Allow).await;
        assert!(matches!(res, Err(PermissionError::AskNotFound)));
    }
}
