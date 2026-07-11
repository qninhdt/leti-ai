//! In-process registry of running subagent tasks.
//!
//! Each `start` returns a [`TaskId`] backed by a [`TaskHandle`] holding
//! the cancellation token, status, output buffer, and cost. Quotas are
//! enforced per ROOT session (every nested descendant counts toward the
//! same root's bucket) so a depth-3 fan-out can't bypass the cap by
//! spreading work across grandchildren.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use rust_decimal::Decimal;

use super::task_types::{
    DEFAULT_MAX_PER_SESSION, MAX_OUTPUT_BYTES, SpawnError, TaskHandle, TaskId, TaskSnapshot,
    TaskStatus,
};
use crate::types::session::SessionId;

/// Decrement an `AtomicUsize` quota counter without ever underflowing.
/// A bare `fetch_sub(1)` on a counter already at 0 wraps to `usize::MAX`,
/// which would permanently wedge the per-root quota (every future
/// `admit` sees `cur >= max` and rejects). This CAS loop floors at 0 so
/// an unbalanced release (double-release, or release racing finalize) is
/// a harmless no-op instead of a permanent spawn lockout.
fn saturating_dec(counter: &AtomicUsize) {
    let mut cur = counter.load(Ordering::Acquire);
    while cur > 0 {
        match counter.compare_exchange_weak(cur, cur - 1, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return,
            Err(actual) => cur = actual,
        }
    }
}

/// Registry of running subagent tasks. Cloneable; interior mutability
/// only (DashMap + Arc).
#[derive(Default, Clone)]
pub struct TaskRegistry {
    tasks: Arc<DashMap<TaskId, TaskHandle>>,
    /// Quota counter per ROOT session. Every started task increments
    /// `session_descendants[root]`; completion / cancellation decrements.
    session_descendants: Arc<DashMap<SessionId, AtomicUsize>>,
    /// Terminal snapshots retained after `finalize` removes the live
    /// entry, so a `poll`/`await_completion` that races behind `finalize`
    /// still returns the real result instead of a spurious "vanished".
    /// Bounded ring — see [`TerminalCache`].
    terminal: Arc<Mutex<TerminalCache>>,
    max_per_session: usize,
}

/// Bounded LRU-ish cache of terminal task snapshots. A subagent that
/// finishes and finalizes before its parent's `await_completion` even
/// looks up the live entry would otherwise see `None` ("task vanished")
/// despite having succeeded. `finalize` records the terminal snapshot
/// here first; lookups fall back to it. Capacity-capped so a long-lived
/// server with many short subagents can't grow it without bound.
#[derive(Default)]
struct TerminalCache {
    map: HashMap<TaskId, TaskSnapshot>,
    order: VecDeque<TaskId>,
}

/// Max terminal snapshots retained. Generous for the await/poll race
/// window (a parent reads its child's result within milliseconds of
/// finalize) while bounding memory on a busy server.
const TERMINAL_CACHE_CAP: usize = 1024;

impl TerminalCache {
    fn insert(&mut self, id: TaskId, snap: TaskSnapshot) {
        if self.map.insert(id, snap).is_none() {
            self.order.push_back(id);
            while self.order.len() > TERMINAL_CACHE_CAP {
                if let Some(evict) = self.order.pop_front() {
                    self.map.remove(&evict);
                }
            }
        }
    }

    fn get(&self, id: TaskId) -> Option<TaskSnapshot> {
        self.map.get(&id).cloned()
    }
}

impl TaskRegistry {
    /// Construct a registry with the supplied quota cap. Tests pass a
    /// small number; production reads `OPENLET_SUBAGENT_MAX_PER_SESSION`
    /// via [`Self::from_env`].
    #[must_use]
    pub fn new(max_per_session: usize) -> Self {
        Self {
            tasks: Arc::new(DashMap::new()),
            session_descendants: Arc::new(DashMap::new()),
            terminal: Arc::new(Mutex::new(TerminalCache::default())),
            max_per_session,
        }
    }

    /// Build a registry honoring the `OPENLET_SUBAGENT_MAX_PER_SESSION`
    /// env override (default [`DEFAULT_MAX_PER_SESSION`]).
    #[must_use]
    pub fn from_env() -> Self {
        let max = std::env::var("OPENLET_SUBAGENT_MAX_PER_SESSION")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(DEFAULT_MAX_PER_SESSION);
        Self::new(max)
    }

    #[must_use]
    pub fn max_per_session(&self) -> usize {
        self.max_per_session
    }

    /// Pre-flight admission check. Increments the counter on success;
    /// caller MUST install the resulting `TaskHandle` via [`Self::insert`]
    /// or release via [`Self::release_quota`] on error.
    pub fn admit(&self, root: SessionId) -> Result<TaskId, SpawnError> {
        let entry = self
            .session_descendants
            .entry(root)
            .or_insert_with(|| AtomicUsize::new(0));
        // CAS loop avoids the false-deny window of fetch_add + rollback:
        // two concurrent admits at cur=max would both observe prev>=max
        // and reject, even though after both rollbacks one slot is free.
        let mut cur = entry.load(Ordering::Acquire);
        loop {
            if cur >= self.max_per_session {
                return Err(SpawnError::SubagentQuotaExceeded {
                    in_flight: cur,
                    max: self.max_per_session,
                });
            }
            match entry.compare_exchange_weak(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Ok(TaskId::new()),
                Err(actual) => cur = actual,
            }
        }
    }

    /// Roll back a quota admit when spawning fails before the handle is
    /// installed (e.g. agent slug unknown).
    pub fn release_quota(&self, root: SessionId) {
        if let Some(c) = self.session_descendants.get(&root) {
            saturating_dec(&c);
        }
    }

    /// Install a `TaskHandle` keyed by `id`. Caller previously claimed
    /// the slot via [`Self::admit`]. Idempotent — the same id replaces
    /// an existing entry, but in practice ids are fresh UUIDs.
    pub fn insert(&self, id: TaskId, handle: TaskHandle) {
        self.tasks.insert(id, handle);
    }

    /// Drop a finished task from the live map and decrement its root's
    /// quota counter. Called from the spawned driver's Drop guard so the
    /// counter releases on success, error, OR panic.
    pub fn finalize(&self, id: TaskId) {
        if let Some((_, handle)) = self.tasks.remove(&id)
            && let Some(c) = self.session_descendants.get(&handle.root_session_id)
        {
            saturating_dec(&c);
        }
    }

    /// Async poll of a task's current snapshot. Falls back to the
    /// terminal cache when the live handle has already been finalized.
    pub async fn poll_async(&self, id: TaskId) -> Option<TaskSnapshot> {
        let Some(handle) = self.tasks.get(&id).map(|h| h.clone()) else {
            // Lost the race with `finalize` — fall back to the terminal cache.
            return self.terminal.lock().unwrap().get(id);
        };
        let status = handle.status.read().await.clone();
        let output = handle.output.read().await.clone();
        let cost = *handle.cost_usd.read().await;
        let finished = status.is_terminal();
        Some(TaskSnapshot {
            task_id: id,
            status,
            output,
            cost_usd: cost,
            finished,
        })
    }

    /// Trip the cancellation token for `id`. Idempotent.
    pub fn cancel(&self, id: TaskId) {
        if let Some(handle) = self.tasks.get(&id) {
            handle.cancel.cancel();
        }
    }

    /// Cascade cancel: every task whose root matches `root` is cancelled.
    /// Driver tasks observe their tokens, finalize, and release quota.
    pub fn cancel_descendants(&self, root: SessionId) {
        for entry in self.tasks.iter() {
            if entry.value().root_session_id == root {
                entry.value().cancel.cancel();
            }
        }
    }

    /// Park until task `id` finishes (status becomes terminal). Returns
    /// `None` if the task id was never installed or was already removed.
    pub async fn await_completion(&self, id: TaskId) -> Option<TaskSnapshot> {
        let Some(handle) = self.tasks.get(&id).map(|h| h.clone()) else {
            // Lost the race: the driver already finalized (removed the
            // live entry) before we looked it up. The terminal snapshot
            // recorded by `set_status` is the authoritative result.
            return self.terminal.lock().unwrap().get(id);
        };
        loop {
            // `Notified` futures are NOT subscribed until polled or
            // explicitly enabled. Without this, a `set_status(terminal)
            // + notify_waiters` racing between the `notified()` call
            // and the await would lose the wakeup and hang forever.
            let notified = handle.finished.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let s = handle.status.read().await;
                if s.is_terminal() {
                    let status = s.clone();
                    drop(s);
                    // Read the snapshot straight from the ALREADY-CLONED
                    // `handle` rather than `self.poll_async(id)`. The driver's
                    // `finalize` may have removed the registry entry between
                    // `set_status(terminal)` and this read; the clone at the
                    // top keeps the `Arc<RwLock<..>>` state alive, so reading
                    // from it returns the completed task's output instead of
                    // racing into a "task vanished" `None`.
                    let output = handle.output.read().await.clone();
                    let cost = *handle.cost_usd.read().await;
                    return Some(TaskSnapshot {
                        task_id: id,
                        status,
                        output,
                        cost_usd: cost,
                        finished: true,
                    });
                }
            }
            notified.await;
            // Re-check on wake — multiple notify_waiters may fire
            // before the status flips terminal.
        }
    }

    /// Append `delta` to the task's output buffer, capped at
    /// [`MAX_OUTPUT_BYTES`]. Once the cap trips, the buffer is replaced
    /// with `[truncated]` so subsequent appends remain bounded.
    pub async fn append_output(&self, id: TaskId, delta: &str) {
        let Some(handle) = self.tasks.get(&id) else {
            return;
        };
        let mut buf = handle.output.write().await;
        if buf.as_str() == "[truncated]" {
            return;
        }
        if buf.len().saturating_add(delta.len()) > MAX_OUTPUT_BYTES {
            *buf = "[truncated]".to_string();
            return;
        }
        buf.push_str(delta);
    }

    /// Replace the status atomically and signal `finished` waiters so
    /// `await_completion` can resume.
    pub async fn set_status(&self, id: TaskId, status: TaskStatus) {
        let Some(handle) = self.tasks.get(&id).map(|h| h.clone()) else {
            return;
        };
        {
            let mut s = handle.status.write().await;
            *s = status.clone();
        }
        // Record the terminal snapshot BEFORE notifying waiters so a
        // racing `finalize` (which removes the live entry) can't strand a
        // poll/await that lost the lookup race — they fall back to this.
        if status.is_terminal() {
            let snap = TaskSnapshot {
                task_id: id,
                status,
                output: handle.output.read().await.clone(),
                cost_usd: *handle.cost_usd.read().await,
                finished: true,
            };
            self.terminal.lock().unwrap().insert(id, snap);
        }
        handle.finished.notify_waiters();
    }

    /// Add `delta` to the task's accumulated cost. Used by the spawn
    /// driver on each provider-billed turn.
    pub async fn add_cost(&self, id: TaskId, delta: Decimal) {
        if let Some(handle) = self.tasks.get(&id) {
            let mut c = handle.cost_usd.write().await;
            *c += delta;
        }
    }
}
