//! Fresh transcript preparation performed immediately before model requests.
//!
//! This is the single seam shared by initial turns, tool continuations and
//! post-compaction turns. It owns reminder collection and active-state
//! filtering so durable history can remain append-only without projecting
//! stale constraints forever.

use std::collections::{HashMap, HashSet};

use chrono::Utc;

use crate::error::{CoreError, FsError};
use crate::projection::{
    LlmMessage, ProjectionCaps, effective_message_ids, project_for_compaction, project_for_llm,
};
use crate::runtime::RuntimeHandles;
use crate::runtime::reminders::{
    ChangedFile, DeliveredReminders, ReminderSnapshot, RuntimeReminder, collect, dedupe_new,
};
use crate::types::message::{Message, MessageId, Role};
use crate::types::part::{Part, PartId, ReminderKind};
use crate::types::permission::PermissionMode;
use crate::types::session::SessionId;

/// Per-request signals owned by the turn loop rather than durable transcript
/// state. A default context is used for the initial request.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReminderRequestContext {
    pub turn_index: usize,
    pub max_turns: usize,
    pub actual_input_tokens: Option<usize>,
    pub context_window: Option<u32>,
}

/// Fresh-load, collect, persist and project one session immediately before a
/// provider request.
pub async fn prepare_session_messages(
    handles: &RuntimeHandles,
    session_id: SessionId,
    caps: ProjectionCaps,
    request: ReminderRequestContext,
) -> Result<Vec<LlmMessage>, CoreError> {
    let (mut messages, mut parts_by_msg) = load_transcript(handles, session_id).await?;
    let snapshot = build_snapshot(handles, session_id, &messages, &parts_by_msg, request).await?;
    let active_candidates = collect(&snapshot, &DeliveredReminders::default());
    let active = active_candidates
        .iter()
        .map(|reminder| (reminder.kind, reminder.stable_key.clone()))
        .collect();
    let delivered = delivered_active_reminders(&messages, &parts_by_msg, &active);
    let candidates = dedupe_new(active_candidates, &delivered);

    if !candidates.is_empty() {
        persist_candidates(handles, session_id, &parts_by_msg, candidates).await?;
        (messages, parts_by_msg) = load_transcript(handles, session_id).await?;
    }

    let projected_parts = filter_to_active_reminders(&messages, &parts_by_msg, &active);
    Ok(project_for_llm(&messages, &projected_parts, caps))
}

/// Prepare the exact transcript for a compaction model request. This shares
/// the normal fresh-load/reminder boundary, then enables only the typed
/// compaction marker's request wording.
pub async fn prepare_compaction_session_messages(
    handles: &RuntimeHandles,
    session_id: SessionId,
    caps: ProjectionCaps,
    request: &str,
) -> Result<Vec<LlmMessage>, CoreError> {
    let (mut messages, mut parts_by_msg) = load_transcript(handles, session_id).await?;
    let snapshot = build_snapshot(
        handles,
        session_id,
        &messages,
        &parts_by_msg,
        ReminderRequestContext::default(),
    )
    .await?;
    let active_candidates = collect(&snapshot, &DeliveredReminders::default());
    let active = active_candidates
        .iter()
        .map(|reminder| (reminder.kind, reminder.stable_key.clone()))
        .collect();
    let delivered = delivered_active_reminders(&messages, &parts_by_msg, &active);
    let candidates = dedupe_new(active_candidates, &delivered);
    if !candidates.is_empty() {
        persist_candidates(handles, session_id, &parts_by_msg, candidates).await?;
        (messages, parts_by_msg) = load_transcript(handles, session_id).await?;
    }
    let projected_parts = filter_to_active_reminders(&messages, &parts_by_msg, &active);
    Ok(project_for_compaction(
        &messages,
        &projected_parts,
        caps,
        request,
    ))
}

async fn load_transcript(
    handles: &RuntimeHandles,
    session_id: SessionId,
) -> Result<(Vec<Message>, HashMap<MessageId, Vec<Part>>), CoreError> {
    let messages = handles.memory.list_messages(session_id).await?;
    let mut parts_by_msg = HashMap::with_capacity(messages.len());
    for message in &messages {
        parts_by_msg.insert(
            message.id,
            handles.memory.list_parts(session_id, message.id).await?,
        );
    }
    Ok((messages, parts_by_msg))
}

async fn build_snapshot(
    handles: &RuntimeHandles,
    session_id: SessionId,
    messages: &[Message],
    parts_by_msg: &HashMap<MessageId, Vec<Part>>,
    request: ReminderRequestContext,
) -> Result<ReminderSnapshot, CoreError> {
    let meta = handles
        .memory
        .get_session(session_id)
        .await?
        .ok_or(crate::error::MemoryError::SessionNotFound)?;
    let effective = effective_message_ids(messages, parts_by_msg);

    let compaction_recovery = parts_by_msg
        .iter()
        .any(|(id, parts)| {
            effective.contains(id) && parts.iter().any(|part| matches!(part, Part::Compaction { .. }))
        })
        .then(|| {
            "Conversation history was compacted. Continue using the active runtime constraints and the generated summary as context.".to_string()
        });

    let runtime_limit = runtime_limit_message(request);
    let exceptional_outcome = latest_exception(messages, parts_by_msg, &effective);

    Ok(ReminderSnapshot {
        permission_mode: Some(permission_mode_label(meta.permission_mode).into()),
        is_subagent: meta.parent_session_id.is_some(),
        compaction_recovery,
        runtime_limit,
        exceptional_outcome,
        changed_files: detect_changed_files(handles, session_id).await,
    })
}

fn runtime_limit_message(request: ReminderRequestContext) -> Option<String> {
    if let (Some(actual), Some(window)) = (request.actual_input_tokens, request.context_window)
        && actual.saturating_mul(100) >= (window as usize).saturating_mul(80)
    {
        return Some(format!(
            "The current request used {actual} of {window} context tokens (at least 80%). Keep subsequent tool output focused."
        ));
    }
    if request.max_turns > 0 && request.turn_index.saturating_add(5) >= request.max_turns {
        return Some(format!(
            "This run is approaching its turn limit: turn {} of {}.",
            request.turn_index, request.max_turns
        ));
    }
    None
}

fn latest_exception(
    messages: &[Message],
    parts_by_msg: &HashMap<MessageId, Vec<Part>>,
    effective: &HashSet<MessageId>,
) -> Option<String> {
    for message in messages.iter().rev() {
        if !effective.contains(&message.id) {
            continue;
        }
        if message.role == Role::Assistant {
            return None;
        }
        let parts = parts_by_msg.get(&message.id)?;
        if message.role == Role::User
            && parts
                .iter()
                .any(|part| !matches!(part, Part::RuntimeReminder { .. }))
        {
            return None;
        }
        if message.role != Role::Tool {
            continue;
        }
        let errors = parts
            .iter()
            .filter_map(|part| match part {
                Part::ToolResult {
                    call_id,
                    ok: false,
                    error,
                    ..
                } => Some(format!(
                    "Tool call {call_id} failed: {}",
                    error.as_deref().unwrap_or("unknown error")
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        return (!errors.is_empty()).then(|| errors.join("\n"));
    }
    None
}

async fn detect_changed_files(handles: &RuntimeHandles, session_id: SessionId) -> Vec<ChangedFile> {
    let observations = handles
        .memory
        .list_observations(session_id)
        .await
        .unwrap_or_default();
    let mut changed = Vec::new();
    for observation in observations {
        let Some(recorded) = observation.fingerprint.as_deref() else {
            continue;
        };
        match handles.fs.read(&observation.path, None).await {
            Ok(bytes) => {
                let current = crate::adapters::memory_store::fingerprint_bytes(&bytes);
                if current != recorded {
                    changed.push(ChangedFile {
                        path: observation.path.to_string_lossy().into_owned(),
                        deleted: false,
                        partial: matches!(
                            observation.scope,
                            crate::adapters::memory_store::ReadScope::Range
                        ),
                        state_key: format!("{recorded}:{current}"),
                    });
                }
            }
            Err(FsError::NotFound(_)) => changed.push(ChangedFile {
                path: observation.path.to_string_lossy().into_owned(),
                deleted: true,
                partial: matches!(
                    observation.scope,
                    crate::adapters::memory_store::ReadScope::Range
                ),
                state_key: format!("{recorded}:deleted"),
            }),
            Err(_) => {}
        }
    }
    changed
}

fn delivered_active_reminders(
    messages: &[Message],
    parts_by_msg: &HashMap<MessageId, Vec<Part>>,
    active: &HashSet<(ReminderKind, String)>,
) -> DeliveredReminders {
    DeliveredReminders::from_effective_parts(
        latest_effective_reminders(messages, parts_by_msg)
            .into_values()
            .filter(|part| {
                matches!(part, Part::RuntimeReminder { reminder_kind, stable_key, .. }
            if active.contains(&(*reminder_kind, stable_key.clone())))
            }),
    )
}

async fn persist_candidates(
    handles: &RuntimeHandles,
    session_id: SessionId,
    raw_parts: &HashMap<MessageId, Vec<Part>>,
    candidates: Vec<RuntimeReminder>,
) -> Result<(), CoreError> {
    let mut max_epoch: HashMap<ReminderKind, u32> = HashMap::new();
    for part in raw_parts.values().flatten() {
        if let Part::RuntimeReminder {
            reminder_kind,
            projection_epoch,
            ..
        } = part
        {
            max_epoch
                .entry(*reminder_kind)
                .and_modify(|epoch| *epoch = (*epoch).max(*projection_epoch))
                .or_insert(*projection_epoch);
        }
    }
    let parts = candidates
        .into_iter()
        .map(|candidate| Part::RuntimeReminder {
            id: PartId::new(),
            reminder_kind: candidate.kind,
            stable_key: candidate.stable_key,
            content: candidate.content,
            projection_epoch: max_epoch
                .get(&candidate.kind)
                .map_or(0, |epoch| epoch.saturating_add(1)),
        })
        .collect::<Vec<_>>();
    let message = Message {
        id: MessageId::new(),
        session_id,
        role: Role::User,
        created_at: Utc::now(),
    };
    let _ = handles
        .memory
        .append_runtime_reminders(session_id, message, parts)
        .await?;
    Ok(())
}

fn filter_to_active_reminders(
    messages: &[Message],
    parts_by_msg: &HashMap<MessageId, Vec<Part>>,
    active: &HashSet<(ReminderKind, String)>,
) -> HashMap<MessageId, Vec<Part>> {
    let latest: HashMap<_, _> = latest_effective_reminders(messages, parts_by_msg)
        .into_iter()
        .map(|(kind, part)| (kind, part.id()))
        .collect();
    parts_by_msg
        .iter()
        .map(|(message_id, parts)| {
            let filtered = parts
                .iter()
                .filter(|part| match part {
                    Part::RuntimeReminder {
                        id,
                        reminder_kind,
                        stable_key,
                        ..
                    } => {
                        if *reminder_kind == ReminderKind::BackgroundTaskSettled {
                            return true;
                        }
                        latest.get(reminder_kind) == Some(id)
                            && active.contains(&(*reminder_kind, stable_key.clone()))
                    }
                    _ => true,
                })
                .cloned()
                .collect();
            (*message_id, filtered)
        })
        .collect()
}

fn latest_effective_reminders<'a>(
    messages: &[Message],
    parts_by_msg: &'a HashMap<MessageId, Vec<Part>>,
) -> HashMap<ReminderKind, &'a Part> {
    let effective = effective_message_ids(messages, parts_by_msg);
    messages
        .iter()
        .filter(|message| effective.contains(&message.id))
        .filter_map(|message| parts_by_msg.get(&message.id))
        .flatten()
        .filter_map(|part| match part {
            Part::RuntimeReminder { reminder_kind, .. } => Some((*reminder_kind, part)),
            _ => None,
        })
        .collect()
}

fn permission_mode_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::ReadOnly => "read_only",
        PermissionMode::WorkspaceWrite => "workspace_write",
        PermissionMode::Danger => "danger",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(session_id: SessionId, role: Role) -> Message {
        Message {
            id: MessageId::new(),
            session_id,
            role,
            created_at: Utc::now(),
        }
    }

    fn reminder(kind: ReminderKind, key: &str, epoch: u32) -> Part {
        Part::RuntimeReminder {
            id: PartId::new(),
            reminder_kind: kind,
            stable_key: key.into(),
            content: key.into(),
            projection_epoch: epoch,
        }
    }

    #[test]
    fn compacted_reminder_does_not_suppress_active_reanchor() {
        let session = SessionId::new();
        let old = message(session, Role::User);
        let compact = message(session, Role::Assistant);
        let messages = vec![old.clone(), compact.clone()];
        let mut parts = HashMap::new();
        parts.insert(
            old.id,
            vec![reminder(
                ReminderKind::ExecutionConstraint,
                "mode:read_only",
                0,
            )],
        );
        parts.insert(
            compact.id,
            vec![Part::Compaction {
                id: PartId::new(),
                summary: "summary".into(),
                compacted_message_ids: vec![old.id.to_string()],
                original_token_count: 100,
            }],
        );
        let snapshot = ReminderSnapshot {
            permission_mode: Some("read_only".into()),
            compaction_recovery: Some("history compacted".into()),
            ..Default::default()
        };

        let active = collect(&snapshot, &DeliveredReminders::default());
        let keys = active
            .iter()
            .map(|candidate| (candidate.kind, candidate.stable_key.clone()))
            .collect();
        let delivered = delivered_active_reminders(&messages, &parts, &keys);
        let candidates = dedupe_new(active, &delivered);
        assert!(candidates.iter().any(|candidate| {
            candidate.kind == ReminderKind::ExecutionConstraint
                && candidate.stable_key == "mode:read_only"
        }));
    }

    #[test]
    fn inactive_constraint_is_removed_from_provider_projection() {
        let session = SessionId::new();
        let old = message(session, Role::User);
        let messages = vec![old.clone()];
        let mut parts = HashMap::new();
        parts.insert(
            old.id,
            vec![reminder(
                ReminderKind::ExecutionConstraint,
                "mode:read_only",
                0,
            )],
        );
        let snapshot = ReminderSnapshot {
            permission_mode: Some("workspace_write".into()),
            ..Default::default()
        };
        let active = collect(&snapshot, &DeliveredReminders::default())
            .into_iter()
            .map(|candidate| (candidate.kind, candidate.stable_key))
            .collect();
        let filtered = filter_to_active_reminders(&messages, &parts, &active);

        assert!(project_for_llm(&messages, &filtered, ProjectionCaps::default()).is_empty());
    }

    #[test]
    fn only_latest_active_reminder_of_each_kind_is_projected() {
        let session = SessionId::new();
        let first = message(session, Role::User);
        let second = message(session, Role::User);
        let messages = vec![first.clone(), second.clone()];
        let mut parts = HashMap::new();
        parts.insert(
            first.id,
            vec![reminder(ReminderKind::RuntimeLimit, "limit:turn 40", 0)],
        );
        parts.insert(
            second.id,
            vec![reminder(ReminderKind::RuntimeLimit, "limit:turn 45", 1)],
        );
        let active = HashSet::from([(ReminderKind::RuntimeLimit, "limit:turn 45".to_string())]);
        let filtered = filter_to_active_reminders(&messages, &parts, &active);
        let projected = project_for_llm(&messages, &filtered, ProjectionCaps::default());

        assert_eq!(projected.len(), 1);
        assert!(projected[0].content.contains("limit:turn 45"));
        assert!(!projected[0].content.contains("limit:turn 40"));
    }

    #[test]
    fn exceptional_outcome_expires_after_a_later_human_turn() {
        let session = SessionId::new();
        let tool = message(session, Role::Tool);
        let user = message(session, Role::User);
        let messages = vec![tool.clone(), user.clone()];
        let mut parts = HashMap::new();
        parts.insert(
            tool.id,
            vec![Part::ToolResult {
                id: PartId::new(),
                call_id: "call-1".into(),
                ok: false,
                text: None,
                error: Some("boom".into()),
            }],
        );
        parts.insert(
            user.id,
            vec![Part::Text {
                id: PartId::new(),
                text: "try something else".into(),
            }],
        );
        let effective = effective_message_ids(&messages, &parts);

        assert_eq!(latest_exception(&messages, &parts, &effective), None);
    }

    #[test]
    fn exceptional_outcome_survives_a_reminder_only_record() {
        let session = SessionId::new();
        let tool = message(session, Role::Tool);
        let runtime = message(session, Role::User);
        let messages = vec![tool.clone(), runtime.clone()];
        let mut parts = HashMap::new();
        parts.insert(
            tool.id,
            vec![Part::ToolResult {
                id: PartId::new(),
                call_id: "call-1".into(),
                ok: false,
                text: None,
                error: Some("boom".into()),
            }],
        );
        parts.insert(
            runtime.id,
            vec![reminder(ReminderKind::RuntimeLimit, "limit", 0)],
        );
        let effective = effective_message_ids(&messages, &parts);

        assert_eq!(
            latest_exception(&messages, &parts, &effective),
            Some("Tool call call-1 failed: boom".into())
        );
    }
}
