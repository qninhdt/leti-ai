//! Tests for `ConfigPermissionMgr` — extracted from `manager.rs` so
//! the production module stays focused on the impl.

#[cfg(test)]
mod tests {
    use super::super::manager::*;
    use leti_core::adapters::permission_manager::PermissionManager;
    use leti_core::error::PermissionError;
    use leti_core::types::permission::{
        AlwaysScope, AskId, Decision, PermissionAction, PermissionCtx, PermissionMode,
        PermissionRequest, PermissionRule,
    };
    use leti_core::types::session::SessionId;

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
        let session_id = SessionId::new();
        m.record_always(
            AlwaysScope::Session { id: session_id },
            PermissionRule {
                permission: "edit:*.md".into(),
                action: PermissionAction::Allow,
            },
        )
        .await
        .unwrap();
        let scoped_ctx = PermissionCtx {
            session_id,
            mode: PermissionMode::WorkspaceWrite,
            interaction_mode: Default::default(),
            ext: Default::default(),
        };
        let d = m.check(scoped_ctx, req("edit:notes.md")).await.unwrap();
        assert!(matches!(d, Decision::Allow));
    }

    #[tokio::test]
    async fn record_always_session_scope_does_not_leak_across_sessions() {
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
        // Different session — rule must not apply.
        let d = m.check(ctx(), req("edit:notes.md")).await.unwrap();
        assert!(matches!(d, Decision::Pending { .. }));
    }

    #[tokio::test]
    async fn record_always_global_scope_applies_everywhere() {
        let m = ConfigPermissionMgr::new();
        m.record_always(
            AlwaysScope::Global,
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

    #[tokio::test]
    async fn accept_ask_uses_original_pattern_not_client_input() {
        // A client cannot persist a broader rule than was
        // shown in the prompt. The pattern comes from the PermissionRequest
        // that produced the ask_id, never from a client-supplied field.
        let m = ConfigPermissionMgr::new();
        let session_id = SessionId::new();
        let scoped_ctx = PermissionCtx {
            session_id,
            mode: PermissionMode::WorkspaceWrite,
            interaction_mode: Default::default(),
            ext: Default::default(),
        };
        // Ask for narrow permission "edit:notes.md".
        let decision = m.check(scoped_ctx, req("edit:notes.md")).await.unwrap();
        let ask_id = match decision {
            Decision::Pending { ask_id } => ask_id,
            other => panic!("expected Pending, got {other:?}"),
        };
        // accept_ask takes scope only — no pattern.
        m.accept_ask(
            ask_id,
            AlwaysScope::Session { id: session_id },
            PermissionAction::Allow,
        )
        .await
        .unwrap();
        // The persisted rule applies to "edit:notes.md".
        let scoped_ctx2 = PermissionCtx {
            session_id,
            mode: PermissionMode::WorkspaceWrite,
            interaction_mode: Default::default(),
            ext: Default::default(),
        };
        let d = m.check(scoped_ctx2, req("edit:notes.md")).await.unwrap();
        assert!(matches!(d, Decision::Allow));
        // But NOT to a broader pattern like "edit:.env".
        let scoped_ctx3 = PermissionCtx {
            session_id,
            mode: PermissionMode::WorkspaceWrite,
            interaction_mode: Default::default(),
            ext: Default::default(),
        };
        let d2 = m.check(scoped_ctx3, req("edit:.env")).await.unwrap();
        assert!(matches!(d2, Decision::Pending { .. }));
    }

    #[tokio::test]
    async fn accept_ask_rejects_workspace_scope() {
        let m = ConfigPermissionMgr::new();
        let session_id = SessionId::new();
        let scoped_ctx = PermissionCtx {
            session_id,
            mode: PermissionMode::WorkspaceWrite,
            interaction_mode: Default::default(),
            ext: Default::default(),
        };
        let decision = m.check(scoped_ctx, req("read:*.rs")).await.unwrap();
        let ask_id = match decision {
            Decision::Pending { ask_id } => ask_id,
            other => panic!("expected Pending, got {other:?}"),
        };
        let res = m
            .accept_ask(
                ask_id,
                AlwaysScope::Workspace {
                    path: "/foo".into(),
                },
                PermissionAction::Allow,
            )
            .await;
        assert!(matches!(res, Err(PermissionError::Unsupported(_))));
    }

    #[tokio::test]
    async fn accept_ask_unknown_returns_expired() {
        let m = ConfigPermissionMgr::new();
        let res = m
            .accept_ask(AskId::new(), AlwaysScope::Global, PermissionAction::Allow)
            .await;
        assert!(matches!(res, Err(PermissionError::AskExpired)));
    }

    #[tokio::test]
    async fn seed_rules_allow_workspace_writes_without_ask() {
        // The boot-time seed grants file ops so the workspace owner isn't
        // prompted on every write/edit.
        let seed = vec![
            PermissionRule {
                permission: "write:**".into(),
                action: PermissionAction::Allow,
            },
            PermissionRule {
                permission: "edit:**".into(),
                action: PermissionAction::Allow,
            },
        ];
        let m = ConfigPermissionMgr::new().with_seed_rules(seed).unwrap();
        assert!(matches!(
            m.check(ctx(), req("edit:/ws/src/main.rs")).await.unwrap(),
            Decision::Allow
        ));
        assert!(matches!(
            m.check(ctx(), req("write:/ws/new.rs")).await.unwrap(),
            Decision::Allow
        ));
    }

    #[tokio::test]
    async fn seed_rules_leave_bash_asking() {
        // bash is intentionally NOT seeded — it still hits the mode default.
        let seed = vec![PermissionRule {
            permission: "write:**".into(),
            action: PermissionAction::Allow,
        }];
        let m = ConfigPermissionMgr::new().with_seed_rules(seed).unwrap();
        assert!(matches!(
            m.check(ctx(), req("bash:ls")).await.unwrap(),
            Decision::Pending { .. }
        ));
    }

    #[tokio::test]
    async fn seed_rules_overridden_by_later_always_deny() {
        // Seeds are the base layer; a persisted always-deny must still win
        // (last-match-wins). record_always appends after the seed.
        let seed = vec![PermissionRule {
            permission: "edit:**".into(),
            action: PermissionAction::Allow,
        }];
        let m = ConfigPermissionMgr::new().with_seed_rules(seed).unwrap();
        m.record_always(
            AlwaysScope::Global,
            PermissionRule {
                permission: "edit:/secret/**".into(),
                action: PermissionAction::Deny,
            },
        )
        .await
        .unwrap();
        assert!(matches!(
            m.check(ctx(), req("edit:/secret/keys.txt")).await.unwrap(),
            Decision::Deny { .. }
        ));
        // Non-secret edits still allowed by the seed.
        assert!(matches!(
            m.check(ctx(), req("edit:/ws/main.rs")).await.unwrap(),
            Decision::Allow
        ));
    }
}
