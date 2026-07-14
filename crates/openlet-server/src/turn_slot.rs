//! Turn-slot claim + driven-turn spawn scaffolding shared by the
//! `prompt_async` and `compact` routes.
//!
//! Both routes do the same concurrency dance: atomically claim the
//! per-session `active_turns` slot (rejecting a concurrent turn), then spawn
//! a task that drives a future to completion under an [`ExitGuard`] +
//! stale-finalizer-safe slot release + status write. Extracted here verbatim
//! so the exact `SlotGuard` commit/drop ordering and finalizer semantics live
//! in one place.

use std::future::Future;
use std::sync::Arc;

use openlet_core::error::CoreError;
use openlet_core::types::session::{SessionId, SessionStatus};
use tokio_util::sync::CancellationToken;

use crate::app_state::{AppState, TurnHandle};
use crate::error::AppError;
use crate::events::publish_status;

/// Drop-guard that releases the `active_turns` slot if any `?` propagates
/// before the slot is committed to the spawned task. Once `committed = true`,
/// the driving task owns slot lifecycle (closes slot leak).
pub(crate) struct SlotGuard<'a> {
    state: &'a AppState,
    sid: SessionId,
    committed: bool,
}

impl SlotGuard<'_> {
    /// Commit the slot to the spawned task — the guard now drops without
    /// releasing the slot.
    pub(crate) fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for SlotGuard<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.state.active_turns.remove(&self.sid);
        }
    }
}

/// Atomically claim the active-turn slot for `sid`, returning the fresh
/// [`TurnHandle`] plus an uncommitted [`SlotGuard`]. `contains_key` then
/// `insert` would let two concurrent callers both pass and one clobber the
/// other, orphaning a running task — the `entry` API closes that race.
/// Rejects with `409 turn_in_flight` when a turn already holds the slot.
pub(crate) fn try_claim_turn_slot(
    state: &AppState,
    sid: SessionId,
) -> Result<(TurnHandle, SlotGuard<'_>), AppError> {
    let handle = TurnHandle::new(sid);
    match state.active_turns.entry(sid) {
        dashmap::mapref::entry::Entry::Occupied(_) => {
            return Err(AppError::conflict(
                "turn_in_flight",
                "a turn is already running for this session",
            ));
        }
        dashmap::mapref::entry::Entry::Vacant(v) => {
            v.insert(handle.clone());
        }
    }
    let guard = SlotGuard {
        state,
        sid,
        committed: false,
    };
    Ok((handle, guard))
}

/// Spawn the task that drives `driver_fut` to completion, owning the
/// full turn lifecycle: an [`ExitGuard`] that notifies `exited` on success /
/// error / panic, a stale-finalizer-safe slot release, and the terminal
/// status write + publish.
///
/// The caller supplies its own driver future (a normal turn loop or an
/// on-demand compaction). `label` names the driver in the error log.
pub(crate) fn spawn_driven_turn<F>(
    state: AppState,
    sid: SessionId,
    handle: TurnHandle,
    label: &'static str,
    driver_fut: F,
) where
    F: Future<Output = Result<(), CoreError>> + Send + 'static,
{
    tokio::spawn(async move {
        // Drop guard ensures `exited` is notified on success, error, OR
        // panic. DELETE/abort awaiters resolve immediately on exit.
        struct ExitGuard(Arc<tokio::sync::Notify>);
        impl Drop for ExitGuard {
            fn drop(&mut self) {
                self.0.notify_waiters();
            }
        }
        let _exit_guard = ExitGuard(handle.exited.clone());

        let cancel = handle.cancel.clone();
        let outcome = driver_fut.await;
        let final_status = match &outcome {
            Ok(_) => SessionStatus::Idle,
            Err(_) if cancel.is_cancelled() => SessionStatus::Cancelled,
            Err(_) => SessionStatus::Errored,
        };
        // Atomically release OUR slot and, if a non-`User` turn is
        // queued, re-claim the slot for it — all inside the same
        // `active_turns` critical section so a racing fresh user prompt
        // (`try_claim_turn_slot`) and a queued injected turn can't both
        // claim. `drained` carries the pending turn to start AFTER the
        // status write below.
        //
        // Stale-finalizer safety: we only act if the slot still holds OUR
        // handle (`Arc::ptr_eq` on `cancel_emitted`). If a fresh prompt
        // already raced past a still-cancelling driver, the slot is not
        // ours — we leave it and drain nothing (the live turn will drain
        // on its own exit).
        let drained = drain_next_turn(&state, sid, &handle);

        let _ = state
            .memory
            .update_status(sid, final_status, status_reason(&outcome, &cancel))
            .await;
        publish_status(&state.events, sid, final_status).await;
        if let Err(err) = outcome {
            tracing::warn!(session = %sid, error = %err, "{label} ended with error");
        }

        // Start the drained injected turn (if any) now that our terminal
        // status is committed. The slot was re-claimed inside
        // `drain_next_turn` so this only commits the new handle to its task.
        if let Some((next_handle, pending)) = drained {
            crate::injected_turn::start_injected_turn(
                state,
                sid,
                next_handle,
                pending.body,
                pending.origin,
            );
        }
    });
}

/// Exit-path drain: release the finishing turn's slot and, atomically,
/// re-claim it for the next queued non-`User` turn if one exists.
///
/// Returns `Some((new_handle, pending))` when a queued turn was dequeued
/// and the slot re-claimed for it (caller must start it), or `None` when
/// the queue was empty (slot released) or the slot was not ours (stale
/// finalizer — left untouched).
///
/// The whole check runs under the `active_turns` `entry` shard lock so it
/// serialises against `try_claim_turn_slot` and `enqueue_or_start_turn`.
fn drain_next_turn(
    state: &AppState,
    sid: SessionId,
    handle: &TurnHandle,
) -> Option<(TurnHandle, crate::app_state::PendingTurn)> {
    match state.active_turns.entry(sid) {
        dashmap::mapref::entry::Entry::Occupied(occ) => {
            // Only OUR handle may release/drain — a fresh user prompt that
            // raced past a still-cancelling driver owns the slot now.
            if !Arc::ptr_eq(&occ.get().cancel_emitted, &handle.cancel_emitted) {
                return None;
            }
            // Pop the next queued turn, if any.
            let next = state
                .pending_turns
                .get_mut(&sid)
                .and_then(|mut q| q.pop_front());
            match next {
                Some(pending) => {
                    // Re-claim the slot in place for the queued turn.
                    let next_handle = TurnHandle::new(sid);
                    occ.replace_entry(next_handle.clone());
                    Some((next_handle, pending))
                }
                None => {
                    // Nothing queued — release the slot.
                    occ.remove();
                    None
                }
            }
        }
        dashmap::mapref::entry::Entry::Vacant(_) => None,
    }
}

/// Map a driver outcome + cancel state to the stable status-reason string
/// persisted alongside the terminal session status.
fn status_reason(outcome: &Result<(), CoreError>, cancel: &CancellationToken) -> &'static str {
    match outcome {
        Ok(_) => "turn finished",
        Err(_) if cancel.is_cancelled() => "cancelled",
        Err(_) => "loop error",
    }
}
