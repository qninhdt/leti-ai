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

use crate::sqlite::permission_repo::{PermissionRecord, PersistedDecision, SqlitePermissionRepo};

use super::ruleset::{CompiledRule, CompiledRuleset};

/// Per-pending-ask state. We carry the request alongside the sender so
/// the API layer can render a user-friendly prompt in the SSE event.
/// `deferred` is held until the runtime calls `take_deferred(ask_id)`,
/// at which point the runtime owns the receiver half of the oneshot.
pub struct PendingAsk {
    pub request: PermissionRequest,
    pub ctx: PermissionCtx,
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
    repo: Option<SqlitePermissionRepo>,
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

    /// Attach a SQLite repo so `accept_ask` persists across restart and
    /// `hydrate` rehydrates rules at boot.
    #[must_use]
    pub fn with_repo(mut self, repo: SqlitePermissionRepo) -> Self {
        self.repo = Some(repo);
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
            repo: None,
        })
    }

    /// Snapshot of pending asks — useful for the HTTP route that lists
    /// open prompts for a session.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Read-only peek at a pending ask's request for SSE rendering.
    pub fn peek_request(&self, ask_id: AskId) -> Option<PermissionRequest> {
        self.pending.get(&ask_id).map(|e| e.request.clone())
    }

    /// Hydrate persisted always-allow rules from the SQLite repo. Called
    /// on boot before any route is mounted, so existing always-allow
    /// rules apply to incoming requests immediately.
    pub async fn hydrate(
        &self,
        sessions: &[openlet_core::types::session::SessionId],
    ) -> Result<(), PermissionError> {
        let Some(repo) = &self.repo else {
            return Ok(());
        };
        let mut g = self.inner.write().await;
        for sid in sessions {
            let records = repo.list_for_session(*sid).await?;
            for rec in records {
                if !matches!(rec.decision, PersistedDecision::Always) {
                    continue;
                }
                let rule = PermissionRule {
                    permission: rec.permission,
                    action: PermissionAction::Allow,
                };
                let scope = AlwaysScope::Session { id: rec.session_id };
                let compiled = CompiledRule::from_rule_scoped(rule, scope)
                    .map_err(|e| PermissionError::Io(e.to_string()))?;
                g.push(compiled);
            }
        }
        Ok(())
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
            g.evaluate(&ctx, &req.permission)
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
                        ctx,
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
        scope: AlwaysScope,
        rule: PermissionRule,
    ) -> Result<(), PermissionError> {
        match &scope {
            AlwaysScope::Global | AlwaysScope::Session { .. } => {}
            AlwaysScope::Workspace { .. } | AlwaysScope::Agent { .. } => {
                return Err(PermissionError::Unsupported(
                    "workspace/agent scope not yet wired".into(),
                ));
            }
        }
        let compiled = CompiledRule::from_rule_scoped(rule, scope)
            .map_err(|e| PermissionError::Io(e.to_string()))?;
        let mut g = self.inner.write().await;
        g.push(compiled);
        Ok(())
    }

    fn take_deferred(&self, ask_id: AskId) -> Option<Deferred<Decision>> {
        self.pending.get_mut(&ask_id)?.deferred.take()
    }

    /// Read-only peek at a pending ask's session id. Used by the HTTP
    /// route to publish `PermissionResolved` to the correct session
    /// before `accept_ask`/`reply` consumes the entry. M7 — this is the
    /// SOLE definition; the duplicate inherent method was removed (all
    /// callers go through `Arc<dyn PermissionManager>`).
    fn peek_session_id(&self, ask_id: AskId) -> Option<openlet_core::types::session::SessionId> {
        self.pending.get(&ask_id).map(|e| e.ctx.session_id)
    }

    async fn accept_ask(
        &self,
        ask_id: AskId,
        scope: AlwaysScope,
        action: PermissionAction,
    ) -> Result<(), PermissionError> {
        match &scope {
            AlwaysScope::Global | AlwaysScope::Session { .. } => {}
            AlwaysScope::Workspace { .. } | AlwaysScope::Agent { .. } => {
                return Err(PermissionError::Unsupported(
                    "workspace/agent scope not yet wired".into(),
                ));
            }
        }
        // Atomic remove — on failure to persist below, we restore.
        let (id, ask) = self
            .pending
            .remove(&ask_id)
            .ok_or(PermissionError::AskExpired)?;
        let rule = PermissionRule {
            permission: ask.request.permission.clone(),
            action,
        };
        let compiled = match CompiledRule::from_rule_scoped(rule.clone(), scope.clone()) {
            Ok(c) => c,
            Err(e) => {
                self.pending.insert(id, ask);
                return Err(PermissionError::Io(e.to_string()));
            }
        };
        if let Some(repo) = &self.repo {
            let record = PermissionRecord {
                session_id: ask.ctx.session_id,
                ask_id,
                permission: ask.request.permission.clone(),
                decision: PersistedDecision::Always,
            };
            if let Err(e) = repo.record(&record).await {
                self.pending.insert(id, ask);
                return Err(e);
            }
        }
        self.inner.write().await.push(compiled);
        // Resolve the in-flight ask with the Decision matching the
        // persisted action — AlwaysDeny must NOT silently allow.
        // `Ask` is invalid here: accept_ask is the user's response to a
        // pending ask, not a re-ask. Reject up front.
        let resolution = match action {
            PermissionAction::Allow => Decision::Allow,
            PermissionAction::Deny => Decision::Deny { feedback: None },
            PermissionAction::Ask => {
                self.pending.insert(id, ask);
                return Err(PermissionError::Unsupported(
                    "accept_ask requires Allow or Deny action".into(),
                ));
            }
        };
        let _ = ask.sender.send(resolution);
        Ok(())
    }
}
