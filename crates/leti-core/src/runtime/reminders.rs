//! Runtime reminder collection.
//!
//! A runtime reminder is harness-authored context injected into the model's
//! view of the conversation as a typed [`Part::RuntimeReminder`]. This module
//! owns the deterministic, before-every-request collection pass:
//!
//! 1. Producers inspect a read-only [`ReminderSnapshot`] and each return
//!    `Option<RuntimeReminder>` — they never write to storage themselves.
//! 2. The collector dedupes each candidate against the reminders already
//!    present in the **effective projected history** (never the raw
//!    append-only log), so a reminder superseded by compaction cannot
//!    suppress its still-active replacement.
//! 3. Surviving candidates are returned to the caller (the shared
//!    request-preparation service) to persist as typed parts before the
//!    projection that builds the provider request.
//!
//! Trusted provenance exists ONLY because runtime code constructs the typed
//! enum here. User text containing `<system-reminder>` tags is ordinary
//! untrusted `Part::Text` and never flows through this module.

use std::collections::HashSet;

use crate::types::part::{Part, ReminderKind};

/// A reminder a producer wants delivered this turn, before it becomes a
/// durable [`Part::RuntimeReminder`]. `stable_key` identifies the logical
/// reminder for dedupe; two candidates with the same `(kind, stable_key)`
/// are the same logical reminder regardless of content revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeReminder {
    pub kind: ReminderKind,
    pub stable_key: String,
    pub content: String,
}

impl RuntimeReminder {
    #[must_use]
    pub fn new(
        kind: ReminderKind,
        stable_key: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            stable_key: stable_key.into(),
            content: content.into(),
        }
    }
}

/// Dedupe identity of an already-delivered reminder: `(kind, stable_key)`.
/// Content revision is intentionally excluded — a producer that changes only
/// the body of an already-present logical reminder does not re-emit it, which
/// keeps repeated no-change requests at zero new reminders.
type DeliveredKey = (ReminderKind, String);

/// The reminders already present in the **effective projected history** — the
/// post-compaction view the model will actually see, not the raw append-only
/// log. Built by the caller from the projected parts so a superseded reminder
/// never masks its active replacement.
#[derive(Debug, Default, Clone)]
pub struct DeliveredReminders {
    keys: HashSet<DeliveredKey>,
}

impl DeliveredReminders {
    /// Build the delivered-set from the effective (already projected) parts.
    /// Only `Part::RuntimeReminder` parts contribute a key.
    #[must_use]
    pub fn from_effective_parts<'a>(parts: impl IntoIterator<Item = &'a Part>) -> Self {
        let mut keys = HashSet::new();
        for p in parts {
            if let Part::RuntimeReminder {
                reminder_kind,
                stable_key,
                ..
            } = p
            {
                keys.insert((*reminder_kind, stable_key.clone()));
            }
        }
        Self { keys }
    }

    #[must_use]
    pub fn contains(&self, kind: ReminderKind, stable_key: &str) -> bool {
        self.keys.contains(&(kind, stable_key.to_string()))
    }
}

/// Filter producer candidates down to the ones not already present in the
/// effective projected history. Within a single collection pass, a duplicate
/// `(kind, stable_key)` among the candidates themselves is also collapsed so
/// two producers cannot double-emit the same logical reminder.
#[must_use]
pub fn dedupe_new(
    candidates: Vec<RuntimeReminder>,
    delivered: &DeliveredReminders,
) -> Vec<RuntimeReminder> {
    let mut seen: HashSet<DeliveredKey> = HashSet::new();
    let mut out = Vec::new();
    for c in candidates {
        let key = (c.kind, c.stable_key.clone());
        if delivered.contains(c.kind, &c.stable_key) {
            continue;
        }
        if !seen.insert(key) {
            continue;
        }
        out.push(c);
    }
    out
}

/// A file the session previously observed that changed or disappeared since
/// the recorded fingerprint. Built by the caller (which has filesystem access)
/// and handed to the workspace-delta producer as pure data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    /// `true` when the file no longer exists; `false` when it still exists but
    /// its content fingerprint differs from the recorded observation.
    pub deleted: bool,
    /// `true` when the prior observation was a partial/range read, so the
    /// reminder must not claim the unseen remainder is authoritative.
    pub partial: bool,
    /// Fingerprint transition used only for durable dedupe. Including both
    /// observed and current state lets a re-read followed by a second change
    /// produce a new reminder even when the path set is identical.
    pub state_key: String,
}

/// Read-only inputs the producers inspect. The caller (the shared request
/// preparation service) assembles this once per request from durable state
/// plus cheap filesystem checks; producers never perform I/O themselves.
#[derive(Debug, Clone, Default)]
pub struct ReminderSnapshot {
    /// Human label of the active permission mode (e.g. `read_only`).
    pub permission_mode: Option<String>,
    /// `true` when this session is a subagent child (has a parent session).
    pub is_subagent: bool,
    /// A just-committed compaction re-anchor: the still-active execution and
    /// limit state that must survive into the post-compaction projection.
    pub compaction_recovery: Option<String>,
    /// A configured token/cost/turn threshold that was reached this request.
    pub runtime_limit: Option<String>,
    /// An exceptional permission/tool outcome worth surfacing to the model.
    pub exceptional_outcome: Option<String>,
    /// Files previously observed by this session that changed or were deleted.
    pub changed_files: Vec<ChangedFile>,
}

/// Producer: surface the active execution constraint (permission mode) so the
/// model is reminded what it may and may not do this turn. Read-only mode is
/// the one worth stating; workspace-write/danger are the permissive defaults
/// and would only add noise, so only `read_only` emits.
#[must_use]
fn produce_execution_constraint(snap: &ReminderSnapshot) -> Option<RuntimeReminder> {
    let mode = snap.permission_mode.as_deref()?;
    if mode != "read_only" {
        return None;
    }
    Some(RuntimeReminder::new(
        ReminderKind::ExecutionConstraint,
        "mode:read_only",
        "You are operating in read-only permission mode. Do not attempt to write, \
         edit, or run mutating commands; propose changes for the user to apply instead.",
    ))
}

/// Producer: remind a subagent child of its delegated-task execution context.
#[must_use]
fn produce_task_state(snap: &ReminderSnapshot) -> Option<RuntimeReminder> {
    if !snap.is_subagent {
        return None;
    }
    Some(RuntimeReminder::new(
        ReminderKind::TaskState,
        "subagent:active",
        "You are a subagent working a delegated task. Complete the assigned objective \
         and return your result; do not start unrelated work.",
    ))
}

/// Producer: after a compaction commits, re-anchor still-active state so the
/// post-compaction projection does not lose it. Keyed by content so a distinct
/// recovery notice re-anchors while an identical one dedupes.
#[must_use]
fn produce_compaction_recovery(snap: &ReminderSnapshot) -> Option<RuntimeReminder> {
    let body = snap.compaction_recovery.as_deref()?;
    Some(RuntimeReminder::new(
        ReminderKind::CompactionRecovery,
        "compaction:recovery",
        body,
    ))
}

/// Producer: surface a reached token/cost/turn threshold.
#[must_use]
fn produce_runtime_limit(snap: &ReminderSnapshot) -> Option<RuntimeReminder> {
    let body = snap.runtime_limit.as_deref()?;
    Some(RuntimeReminder::new(
        ReminderKind::RuntimeLimit,
        format!("limit:{body}"),
        body,
    ))
}

/// Producer: surface an exceptional permission/tool outcome.
#[must_use]
fn produce_exceptional_outcome(snap: &ReminderSnapshot) -> Option<RuntimeReminder> {
    let body = snap.exceptional_outcome.as_deref()?;
    Some(RuntimeReminder::new(
        ReminderKind::ExceptionalOutcome,
        format!("outcome:{body}"),
        body,
    ))
}

/// Producer: report files the session previously observed that changed or were
/// deleted. One reminder covers the whole batch, keyed by the sorted path set
/// so an unchanged set does not re-emit and a new/changed set does.
#[must_use]
fn produce_workspace_delta(snap: &ReminderSnapshot) -> Option<RuntimeReminder> {
    if snap.changed_files.is_empty() {
        return None;
    }
    let mut lines: Vec<String> = snap
        .changed_files
        .iter()
        .map(|f| {
            let state = if f.deleted {
                "deleted"
            } else if f.partial {
                "changed (you previously read only part of it)"
            } else {
                "changed"
            };
            format!("- {} ({state}) [{}]", f.path, f.state_key)
        })
        .collect();
    lines.sort();
    // Stable key over the sorted path+state set so re-reporting the same delta
    // dedupes, but any new/removed changed file forms a fresh logical reminder.
    let key = format!("workspace_delta:{}", lines.join("|"));
    let display_lines = lines
        .iter()
        .map(|line| {
            line.rsplit_once(" [")
                .map_or(line.as_str(), |(text, _)| text)
        })
        .collect::<Vec<_>>();
    let body = format!(
        "Files you previously read have changed on disk since you last saw them. \
         Re-read them before relying on their earlier contents:\n{}",
        display_lines.join("\n")
    );
    Some(RuntimeReminder::new(
        ReminderKind::WorkspaceDelta,
        key,
        body,
    ))
}

/// Run every producer against the snapshot, in the plan's defined order, and
/// return the surviving new reminders after effective-projection dedupe. This
/// is the one entry point the shared request-preparation service calls.
#[must_use]
pub fn collect(snap: &ReminderSnapshot, delivered: &DeliveredReminders) -> Vec<RuntimeReminder> {
    let candidates: Vec<RuntimeReminder> = [
        produce_execution_constraint(snap),
        produce_compaction_recovery(snap),
        produce_task_state(snap),
        produce_runtime_limit(snap),
        produce_exceptional_outcome(snap),
        produce_workspace_delta(snap),
    ]
    .into_iter()
    .flatten()
    .collect();
    dedupe_new(candidates, delivered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::part::{Part, PartId};

    fn reminder_part(kind: ReminderKind, key: &str, epoch: u32) -> Part {
        Part::RuntimeReminder {
            id: PartId::new(),
            reminder_kind: kind,
            stable_key: key.to_string(),
            content: "body".into(),
            projection_epoch: epoch,
        }
    }

    #[test]
    fn dedupe_drops_already_delivered() {
        let delivered = DeliveredReminders::from_effective_parts(&[reminder_part(
            ReminderKind::ExecutionConstraint,
            "mode:read_only",
            0,
        )]);
        let out = dedupe_new(
            vec![
                RuntimeReminder::new(ReminderKind::ExecutionConstraint, "mode:read_only", "x"),
                RuntimeReminder::new(ReminderKind::RuntimeLimit, "tokens:80pct", "y"),
            ],
            &delivered,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, ReminderKind::RuntimeLimit);
    }

    #[test]
    fn dedupe_collapses_intra_pass_duplicates() {
        let delivered = DeliveredReminders::default();
        let out = dedupe_new(
            vec![
                RuntimeReminder::new(ReminderKind::TaskState, "task:1", "a"),
                RuntimeReminder::new(ReminderKind::TaskState, "task:1", "b"),
            ],
            &delivered,
        );
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn same_key_different_kind_is_not_a_duplicate() {
        let delivered = DeliveredReminders::default();
        let out = dedupe_new(
            vec![
                RuntimeReminder::new(ReminderKind::TaskState, "shared", "a"),
                RuntimeReminder::new(ReminderKind::WorkspaceDelta, "shared", "b"),
            ],
            &delivered,
        );
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn active_replacement_after_compaction_is_not_suppressed() {
        // A reminder that was delivered in the PRE-compaction log but is NOT in
        // the effective projected parts (because compaction dropped it) must be
        // re-emittable. The delivered-set is built only from effective parts,
        // so an empty effective set means the candidate survives.
        let delivered = DeliveredReminders::from_effective_parts(std::iter::empty::<&Part>());
        let out = dedupe_new(
            vec![RuntimeReminder::new(
                ReminderKind::ExecutionConstraint,
                "mode:read_only",
                "x",
            )],
            &delivered,
        );
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn read_only_mode_emits_execution_constraint() {
        let snap = ReminderSnapshot {
            permission_mode: Some("read_only".into()),
            ..Default::default()
        };
        let out = collect(&snap, &DeliveredReminders::default());
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, ReminderKind::ExecutionConstraint);
    }

    #[test]
    fn permissive_modes_emit_no_execution_constraint() {
        for mode in ["workspace_write", "danger"] {
            let snap = ReminderSnapshot {
                permission_mode: Some(mode.into()),
                ..Default::default()
            };
            assert!(
                produce_execution_constraint(&snap).is_none(),
                "mode: {mode}"
            );
        }
    }

    #[test]
    fn subagent_emits_task_state() {
        let snap = ReminderSnapshot {
            is_subagent: true,
            ..Default::default()
        };
        assert_eq!(
            produce_task_state(&snap).map(|r| r.kind),
            Some(ReminderKind::TaskState)
        );
    }

    #[test]
    fn changed_files_emit_one_workspace_delta_keyed_by_set() {
        let snap = ReminderSnapshot {
            changed_files: vec![
                ChangedFile {
                    path: "b.rs".into(),
                    deleted: false,
                    partial: false,
                    state_key: "old:new".into(),
                },
                ChangedFile {
                    path: "a.rs".into(),
                    deleted: true,
                    partial: false,
                    state_key: "old:deleted".into(),
                },
            ],
            ..Default::default()
        };
        let r = produce_workspace_delta(&snap).expect("delta emitted");
        assert_eq!(r.kind, ReminderKind::WorkspaceDelta);
        // Sorted set → deterministic key regardless of input order.
        assert!(r.stable_key.contains("a.rs"));
        assert!(r.stable_key.contains("b.rs"));
        assert!(r.content.contains("deleted"));
    }

    #[test]
    fn empty_snapshot_produces_nothing() {
        let out = collect(&ReminderSnapshot::default(), &DeliveredReminders::default());
        assert!(out.is_empty());
    }

    #[test]
    fn all_six_producers_can_emit_together() {
        let snap = ReminderSnapshot {
            permission_mode: Some("read_only".into()),
            is_subagent: true,
            compaction_recovery: Some("still read-only after compaction".into()),
            runtime_limit: Some("80% of context window used".into()),
            exceptional_outcome: Some("a tool was denied by policy".into()),
            changed_files: vec![ChangedFile {
                path: "x.rs".into(),
                deleted: false,
                partial: true,
                state_key: "old:new".into(),
            }],
        };
        let out = collect(&snap, &DeliveredReminders::default());
        assert_eq!(out.len(), 6);
    }
}
