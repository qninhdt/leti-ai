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
        // Remove ONLY our own handle. If a fresh prompt raced past a
        // still-cancelling driver, this `remove_if` is a no-op so the
        // dying loop's tail finalizer can't stomp the new turn's slot
        // (closes stale-finalizer race).
        state.active_turns.remove_if(&sid, |_, h| {
            Arc::ptr_eq(&h.cancel_emitted, &handle.cancel_emitted)
        });
        let _ = state
            .memory
            .update_status(sid, final_status, status_reason(&outcome, &cancel))
            .await;
        publish_status(&state.events, sid, final_status).await;
        if let Err(err) = outcome {
            tracing::warn!(session = %sid, error = %err, "{label} ended with error");
        }
    });
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
