//! `QuestionRegistry` — single-use rendezvous between an `ask_user` tool
//! invocation and the eventual `POST /v1/sessions/:id/question/answer`
//! reply.
//!
//! The tool registers a fresh [`QuestionId`] (UUIDv7) plus a
//! [`tokio::sync::oneshot::Sender`]; the REST handler resolves it once,
//! transferring ownership of the sender out of the map atomically. A
//! second resolve attempt for the same id surfaces
//! [`ResolveError::NotFound`] — replay of an already-answered question
//! never re-fires the tool.
//!
//! Cancellation drops the sender, which closes the receiver and lets
//! the awaiting tool observe the timeout/cancel branch.

use std::sync::Arc;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::types::session::SessionId;

/// Strongly-typed question identifier (UUIDv7 — sortable by issue time).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct QuestionId(pub Uuid);

impl QuestionId {
    /// Mint a fresh UUIDv7-based id. Time-ordered so registry entries
    /// inserted close together stay clustered, which keeps DashMap
    /// shard locality reasonable under load.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl Default for QuestionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for QuestionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<Uuid> for QuestionId {
    fn from(v: Uuid) -> Self {
        Self(v)
    }
}

/// Failure modes for [`QuestionRegistry::resolve`]. Single-use semantics
/// mean the only happy path returns `Ok(())`; everything else maps to a
/// stable variant the REST layer can translate to an HTTP status.
#[derive(Debug, Error)]
pub enum ResolveError {
    /// No registered sender for this id. Either the question was never
    /// registered (typo / wrong session), already answered, or was
    /// cancelled before the answer arrived.
    #[error("question_not_found")]
    NotFound,
    /// The awaiting tool already dropped its receiver (timeout, cancel,
    /// session shutdown). The registry entry is removed in that case so
    /// subsequent resolves still report `NotFound` — keeps the public
    /// behaviour consistent regardless of which side gave up first.
    #[error("question_receiver_dropped")]
    ReceiverDropped,
    /// The path session id doesn't match the session that registered
    /// this question. The HTTP route asserts this so an attacker who
    /// guessed (or sniffed) a `question_id` for session B can't resolve
    /// it through `/v1/sessions/A/question/answer`.
    #[error("question_session_mismatch")]
    SessionMismatch,
}

/// Cancellation reason. Forwarded into the failure branch of the
/// `ask_user` tool so the model sees a structured error code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelReason {
    /// Session is being torn down (DELETE / abort / shutdown).
    SessionEnding,
    /// Generic operator-driven cancel.
    Operator,
}

struct PendingEntry {
    session_id: SessionId,
    sender: oneshot::Sender<Vec<usize>>,
}

#[derive(Default)]
struct Inner {
    pending: DashMap<QuestionId, PendingEntry>,
    /// Set of sessions with an in-flight question. The `ask_user` tool
    /// caps this at 1 per session to prevent the model from queueing a
    /// stack of modal prompts the user can't reasonably answer in order.
    /// M6 — modeled as a Set (`()` value) rather than a counter: the cap
    /// is "at most one pending question per session", so membership is the
    /// only state. A `u8` count invited a decrement-vs-remove ambiguity;
    /// a Set has exactly one mutation (insert on claim, remove on release).
    pending_per_session: DashMap<SessionId, ()>,
}

/// Process-wide registry of in-flight `ask_user` questions. Cloning is
/// cheap — the inner [`DashMap`] sits behind an [`Arc`].
#[derive(Clone, Default)]
pub struct QuestionRegistry {
    inner: Arc<Inner>,
}

impl QuestionRegistry {
    /// Construct an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a fresh sender keyed by `qid`, scoped to `session_id`.
    /// `resolve` later verifies the caller's path session matches —
    /// without that check, a question_id leaked from session B could be
    /// answered via `/v1/sessions/A/question/answer`. Replacing an
    /// existing key is a programmer bug — UUIDv7 ids must be unique by
    /// construction — but we tolerate it by closing the previous sender
    /// so its awaiter observes the cancel branch instead of hanging
    /// forever.
    pub fn register(
        &self,
        qid: QuestionId,
        session_id: SessionId,
        sender: oneshot::Sender<Vec<usize>>,
    ) {
        if let Some((_, prev)) = self.inner.pending.remove(&qid) {
            drop(prev.sender);
        }
        self.inner
            .pending
            .insert(qid, PendingEntry { session_id, sender });
    }

    /// Single-use resolve scoped by session. The first call removes the
    /// entry and forwards `selected` to the waiting tool IFF
    /// `expected_session` matches the registration. A mismatch returns
    /// the entry to the map and reports
    /// [`ResolveError::SessionMismatch`] without firing the receiver —
    /// defends against cross-session guessing of `question_id`. A second
    /// call returns [`ResolveError::NotFound`].
    pub fn resolve(
        &self,
        qid: QuestionId,
        expected_session: SessionId,
        selected: Vec<usize>,
    ) -> Result<(), ResolveError> {
        let (_, entry) = self
            .inner
            .pending
            .remove(&qid)
            .ok_or(ResolveError::NotFound)?;
        if entry.session_id != expected_session {
            self.inner.pending.insert(qid, entry);
            return Err(ResolveError::SessionMismatch);
        }
        entry
            .sender
            .send(selected)
            .map_err(|_| ResolveError::ReceiverDropped)
    }

    /// Cancel a pending question. Drops the sender so the awaiting tool
    /// observes its receiver close (which the tool maps to a structured
    /// `question_cancelled` error). Idempotent — cancelling an already
    /// resolved/cancelled id is a no-op.
    pub fn cancel(&self, qid: QuestionId, _reason: CancelReason) {
        if let Some((_, entry)) = self.inner.pending.remove(&qid) {
            drop(entry.sender);
        }
    }

    /// Test/diagnostic helper — number of pending entries. Not a load
    /// metric: the registry exposes this to keep tests succinct, not as
    /// a stable runtime API.
    #[must_use]
    pub fn pending_len(&self) -> usize {
        self.inner.pending.len()
    }

    /// Try to claim the per-session pending slot. Returns `true` when
    /// the caller now holds the slot; `false` when another question is
    /// already in flight for the same session. Callers MUST pair a
    /// successful claim with [`Self::remove_session_slot`] after the
    /// question resolves, times out, or is cancelled — even on the
    /// error path.
    pub fn try_claim_session_slot(&self, session_id: SessionId) -> bool {
        // `entry` returns a guard that lets us inspect-or-insert under
        // the shard lock so two concurrent claims can't both observe
        // "no entry" and both flip to present.
        use dashmap::mapref::entry::Entry;
        match self.inner.pending_per_session.entry(session_id) {
            Entry::Occupied(_) => false,
            Entry::Vacant(slot) => {
                slot.insert(());
                true
            }
        }
    }

    /// Remove a previously claimed per-session slot. M6 — the cap is a
    /// Set, so "release" is a single `remove`; there is no count to
    /// decrement and thus no decrement-vs-remove ambiguity. Idempotent —
    /// if the slot was already removed (because cancellation raced with
    /// resolution) this is a no-op.
    pub fn remove_session_slot(&self, session_id: SessionId) {
        self.inner.pending_per_session.remove(&session_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolve_single_use() {
        let reg = QuestionRegistry::new();
        let qid = QuestionId::new();
        let session = SessionId::new();
        let (tx, mut rx) = oneshot::channel::<Vec<usize>>();
        reg.register(qid, session, tx);

        // First resolve delivers payload + drains the entry.
        reg.resolve(qid, session, vec![0])
            .expect("first resolve succeeds");
        assert_eq!(rx.try_recv().expect("payload arrived"), vec![0]);
        assert_eq!(reg.pending_len(), 0);

        // Replay → NotFound, never re-fires the receiver.
        let err = reg
            .resolve(qid, session, vec![1])
            .expect_err("second resolve must fail");
        assert!(matches!(err, ResolveError::NotFound));
    }

    #[tokio::test]
    async fn resolve_rejects_session_mismatch() {
        let reg = QuestionRegistry::new();
        let qid = QuestionId::new();
        let owner = SessionId::new();
        let attacker = SessionId::new();
        let (tx, mut rx) = oneshot::channel::<Vec<usize>>();
        reg.register(qid, owner, tx);

        // Attacker's path session doesn't match the registration; the
        // entry must be returned to the map and the awaiter must NOT
        // observe a value.
        let err = reg
            .resolve(qid, attacker, vec![1])
            .expect_err("attacker path must reject");
        assert!(matches!(err, ResolveError::SessionMismatch));
        assert_eq!(reg.pending_len(), 1, "entry preserved on mismatch");
        // Receiver still empty — sender wasn't fired.
        assert!(rx.try_recv().is_err());

        // Legitimate caller still resolves normally.
        reg.resolve(qid, owner, vec![7])
            .expect("legitimate resolve succeeds");
        assert_eq!(rx.try_recv().expect("payload arrived"), vec![7]);
    }

    #[tokio::test]
    async fn cancel_drops_sender_and_unblocks_awaiter() {
        let reg = QuestionRegistry::new();
        let qid = QuestionId::new();
        let session = SessionId::new();
        let (tx, rx) = oneshot::channel::<Vec<usize>>();
        reg.register(qid, session, tx);

        reg.cancel(qid, CancelReason::SessionEnding);
        // Receiver should observe the closed channel rather than hanging.
        assert!(rx.await.is_err());
        assert_eq!(reg.pending_len(), 0);

        // Cancelling again is a no-op.
        reg.cancel(qid, CancelReason::Operator);
    }

    #[tokio::test]
    async fn resolve_after_receiver_drop_reports_error() {
        let reg = QuestionRegistry::new();
        let qid = QuestionId::new();
        let session = SessionId::new();
        let (tx, rx) = oneshot::channel::<Vec<usize>>();
        reg.register(qid, session, tx);
        drop(rx);

        let err = reg
            .resolve(qid, session, vec![0])
            .expect_err("receiver dropped, resolve must fail");
        assert!(matches!(err, ResolveError::ReceiverDropped));
    }

    #[test]
    fn question_id_is_uuid_v7() {
        let qid = QuestionId::new();
        // UUIDv7 sets version field to 7 in the high nibble of byte 6.
        let uuid = qid.as_uuid();
        let bytes = uuid.as_bytes();
        assert_eq!(bytes[6] >> 4, 0x7, "expected UUIDv7 (got bytes={bytes:?})");
    }
}
