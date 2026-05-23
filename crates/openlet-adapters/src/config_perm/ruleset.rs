//! Ruleset shape + glob-pattern compilation.
//!
//! A rule is `{ permission, pattern, action }`. `permission` is the
//! `<verb>:<target>` string the tool emits (e.g. `read:src/main.rs`,
//! `bash:rm -rf /`); `pattern` is a glob applied via `globset` against
//! that string; `action` is `allow|ask|deny`. Last-match-wins per
//! brainstorm — we explicitly diverge from claw-code's first-match.

use globset::{Glob, GlobMatcher};
use openlet_core::types::permission::{PermissionAction, PermissionRule};

/// One compiled rule. We pre-compile the glob to keep the hot path fast;
/// re-compilation only happens when the ruleset is edited (config reload
/// or `record_always`).
#[derive(Debug, Clone)]
pub(crate) struct CompiledRule {
    pub permission_glob: GlobMatcher,
    pub action: PermissionAction,
    #[allow(dead_code)] // surfaced via API in phase 5
    pub source: PermissionRule,
}

impl CompiledRule {
    pub(crate) fn from_rule(rule: PermissionRule) -> Result<Self, globset::Error> {
        let glob = Glob::new(&rule.permission)?.compile_matcher();
        Ok(Self {
            permission_glob: glob,
            action: rule.action,
            source: rule,
        })
    }

    pub(crate) fn matches(&self, permission: &str) -> bool {
        self.permission_glob.is_match(permission)
    }
}

/// A compiled ruleset — last-match-wins. Layered per amendment §E
/// (defaults ++ agent ++ workspace ++ session) by concatenating
/// `CompiledRuleset`s before evaluation.
#[derive(Debug, Default, Clone)]
pub(crate) struct CompiledRuleset {
    pub rules: Vec<CompiledRule>,
}

impl CompiledRuleset {
    pub(crate) fn from_rules(rules: Vec<PermissionRule>) -> Result<Self, globset::Error> {
        let mut compiled = Vec::with_capacity(rules.len());
        for r in rules {
            compiled.push(CompiledRule::from_rule(r)?);
        }
        Ok(Self { rules: compiled })
    }

    #[allow(dead_code)] // wired in phase 4C when layered ruleset lands
    pub(crate) fn append(&mut self, other: CompiledRuleset) {
        self.rules.extend(other.rules);
    }

    pub(crate) fn push(&mut self, rule: CompiledRule) {
        self.rules.push(rule);
    }

    /// Last-match-wins lookup. Returns the action of the last matching
    /// rule, or `None` if no rule matches (caller falls back to mode).
    pub(crate) fn evaluate(&self, permission: &str) -> Option<&CompiledRule> {
        self.rules.iter().rev().find(|r| r.matches(permission))
    }
}
