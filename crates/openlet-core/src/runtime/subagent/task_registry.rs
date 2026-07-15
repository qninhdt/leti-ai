//! In-process registry of running subagent tasks.
//!
//! Each `start` returns a [`TaskId`] backed by a [`TaskHandle`] holding
//! the cancellation token, status, output buffer, and cost. Quotas are
//! enforced per ROOT session (every nested descendant counts toward the
//! same root's bucket) so a depth-3 fan-out can't bypass the cap by
//! spreading work across grandchildren.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use rust_decimal::Decimal;

use super::task_types::{
    DEFAULT_MAX_INBOX_DEPTH, DEFAULT_MAX_LIFETIME_SPAWNS, DEFAULT_MAX_MESSAGE_BYTES,
    DEFAULT_MAX_PER_SESSION, DeliveryOwnership, HandleName, MAX_OUTPUT_BYTES, RosterEntry,
    SpawnError, TaskHandle, TaskId, TaskSnapshot, TaskStatus,
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
///
/// NOT `Default`: a zero-valued registry would set `max_per_session` and
/// `max_lifetime_spawns` to 0, fail-closing EVERY `admit`. Construct via
/// [`Self::new`], [`Self::with_limits`], or [`Self::from_env`], which set
/// real caps.
#[derive(Clone)]
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
    /// Live task-to-child-session association, published before the driver
    /// starts so callers can navigate a foreground child while it is running.
    child_sessions: Arc<DashMap<TaskId, SessionId>>,
    max_per_session: usize,
    /// CUMULATIVE spawn counter per ROOT session — distinct from the
    /// concurrency counter (`session_descendants`, which decrements on
    /// finalize). This one only ever increments, capping the TOTAL number
    /// of subagents a root may ever spawn over its lifetime. It fail-closes
    /// the Phase 3 injection-driven runaway ("the injected result says
    /// spawn 32 more, whose results inject and spawn 32 more…") that the
    /// concurrency cap alone cannot stop, since each generation finalizes
    /// before the next admits. Overridable via
    /// `OPENLET_SUBAGENT_LIFETIME_BUDGET`.
    lifetime_spawns: Arc<DashMap<SessionId, AtomicUsize>>,
    max_lifetime_spawns: usize,
    /// Sibling roster (Phase 4). Keyed by ROOT session; inner map keyed by
    /// the UNIQUE handle name so two same-slug siblings (`reviewer`,
    /// `reviewer#2`) never collide. Entries survive `finalize` only while
    /// the task is background-alive; `remove_from_roster` drops one when it
    /// is no longer addressable, so a `send_message` to a finished sibling
    /// gets a typed "not addressable" error rather than a silent misroute.
    roster: Arc<DashMap<SessionId, HashMap<HandleName, RosterEntry>>>,
    /// Monotonic generation counter for roster (re)binds. Every
    /// `register_name` stamps the entry with the next value; a
    /// `send_message` carrying a stale `gen` snapshot is refused
    /// (name-safety generation check — a recycled name can't misroute).
    roster_gen: Arc<AtomicU64>,
    /// Max messages buffered per task inbox before `push_message` rejects
    /// (depth bound — a chatty sender can't grow memory without limit).
    max_inbox_depth: usize,
    /// Max bytes per message body accepted by `push_message` (length bound
    /// — an adversarial sender can't smuggle a huge payload past the
    /// per-task output cap via the inbox).
    max_message_bytes: usize,
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

/// Result of trying to hand a running foreground task to the background
/// delivery path. This is deliberately separate from `TaskStatus`: a task can
/// still be running while its output owner has changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundTransition {
    Backgrounded,
    AlreadyBackground,
    AlreadyTerminal,
    NotFound,
}

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
        Self::with_limits(max_per_session, DEFAULT_MAX_LIFETIME_SPAWNS)
    }

    /// Construct with both the concurrency cap AND the per-root cumulative
    /// lifetime spawn budget. The lifetime budget is DISTINCT from
    /// concurrency: it counts EVERY admit under a root over the root's
    /// whole life (never decremented on finalize), fail-closing a runaway
    /// injection-driven spawn loop (Phase 3 Finding 15) that would
    /// otherwise churn within the concurrency cap forever.
    #[must_use]
    pub fn with_limits(max_per_session: usize, max_lifetime_spawns: usize) -> Self {
        Self {
            tasks: Arc::new(DashMap::new()),
            session_descendants: Arc::new(DashMap::new()),
            lifetime_spawns: Arc::new(DashMap::new()),
            terminal: Arc::new(Mutex::new(TerminalCache::default())),
            child_sessions: Arc::new(DashMap::new()),
            max_per_session,
            max_lifetime_spawns,
            roster: Arc::new(DashMap::new()),
            roster_gen: Arc::new(AtomicU64::new(0)),
            max_inbox_depth: DEFAULT_MAX_INBOX_DEPTH,
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
        }
    }

    /// Construct with explicit inbox bounds — for tests that exercise the
    /// depth / per-message-length caps without setting env vars. The
    /// concurrency and lifetime caps use the supplied `max_per_session` and
    /// the default lifetime budget.
    #[must_use]
    pub fn with_message_limits(
        max_per_session: usize,
        max_inbox_depth: usize,
        max_message_bytes: usize,
    ) -> Self {
        let mut reg = Self::with_limits(max_per_session, DEFAULT_MAX_LIFETIME_SPAWNS);
        reg.max_inbox_depth = max_inbox_depth;
        reg.max_message_bytes = max_message_bytes;
        reg
    }

    /// Build a registry honoring the `OPENLET_SUBAGENT_MAX_PER_SESSION`
    /// and `OPENLET_SUBAGENT_MAX_LIFETIME_SPAWNS` env overrides (defaults
    /// [`DEFAULT_MAX_PER_SESSION`] / [`DEFAULT_MAX_LIFETIME_SPAWNS`]).
    #[must_use]
    pub fn from_env() -> Self {
        let max = std::env::var("OPENLET_SUBAGENT_MAX_PER_SESSION")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(DEFAULT_MAX_PER_SESSION);
        let lifetime = std::env::var("OPENLET_SUBAGENT_MAX_LIFETIME_SPAWNS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(DEFAULT_MAX_LIFETIME_SPAWNS);
        Self::with_limits(max, lifetime)
    }

    #[must_use]
    pub fn max_per_session(&self) -> usize {
        self.max_per_session
    }

    /// Pre-flight admission check. Increments the counter on success;
    /// caller MUST install the resulting `TaskHandle` via [`Self::insert`]
    /// or release via [`Self::release_quota`] on error.
    pub fn admit(&self, root: SessionId) -> Result<TaskId, SpawnError> {
        // Lifetime budget check FIRST (cheap, monotonic). This counter is
        // never decremented, so once a root has spawned `max_lifetime_spawns`
        // descendants over its whole life, further admits fail closed —
        // caps a runaway injection-driven spawn loop that would otherwise
        // churn forever inside the concurrency cap (Phase 3 Finding 15).
        {
            let life = self
                .lifetime_spawns
                .entry(root)
                .or_insert_with(|| AtomicUsize::new(0));
            let spent = life.fetch_add(1, Ordering::AcqRel);
            if spent >= self.max_lifetime_spawns {
                // Undo our increment so the count reflects genuine admits
                // (it stays saturated at the cap, not creeping past it).
                saturating_dec(&life);
                return Err(SpawnError::SubagentLifetimeBudgetExceeded {
                    spawned: spent,
                    max: self.max_lifetime_spawns,
                });
            }
        }

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
                // Concurrency-rejected: refund the lifetime increment (this
                // admit did not produce a live task, so it shouldn't count
                // against the lifetime budget).
                if let Some(life) = self.lifetime_spawns.get(&root) {
                    saturating_dec(&life);
                }
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

    pub fn link_child(&self, id: TaskId, child_session_id: SessionId) {
        self.child_sessions.insert(id, child_session_id);
    }

    #[must_use]
    pub fn child_session(&self, id: TaskId) -> Option<SessionId> {
        self.child_sessions.get(&id).map(|session| *session)
    }

    /// Drop a finished task from the live map and decrement its root's
    /// quota counter. Called explicitly by the subagent driver at the tail
    /// of its lifecycle (after the re-arm loop settles). NOTE: this is an
    /// explicit call, NOT a Drop guard — an early return or panic in the
    /// driver BEFORE this point leaks the slot. `saturating_dec` keeps a
    /// double-call harmless; the leak risk is a missing call, tracked by
    /// the `panic_between_admit_and_finalize_leaks_quota_slot` regression.
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

    /// Atomically transfer terminal-output ownership from a foreground waiter
    /// to the durable background outbox. A settlement racing this operation
    /// wins by changing the same CAS word to a terminal state; in that case
    /// foreground remains the sole owner of the original tool result.
    pub fn background_task(
        &self,
        id: TaskId,
        parent_session_id: SessionId,
    ) -> BackgroundTransition {
        let Some(handle) = self.tasks.get(&id).map(|h| h.clone()) else {
            return if self.terminal.lock().unwrap().get(id).is_some() {
                BackgroundTransition::AlreadyTerminal
            } else {
                BackgroundTransition::NotFound
            };
        };
        if handle.parent_session_id != parent_session_id {
            return BackgroundTransition::NotFound;
        }
        match handle.delivery.compare_exchange(
            DeliveryOwnership::ForegroundWaiting.as_u8(),
            DeliveryOwnership::Background.as_u8(),
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                // Wake a foreground `await_completion` so its blocked tool
                // call returns its running acknowledgement immediately.
                handle.finished.notify_waiters();
                BackgroundTransition::Backgrounded
            }
            Err(value) => match DeliveryOwnership::from_u8(value) {
                DeliveryOwnership::Background => BackgroundTransition::AlreadyBackground,
                DeliveryOwnership::TerminalForeground | DeliveryOwnership::TerminalBackground => {
                    BackgroundTransition::AlreadyTerminal
                }
                DeliveryOwnership::ForegroundWaiting => unreachable!("CAS returned expected value"),
            },
        }
    }

    /// Resolve which path owns settlement. Must run before publishing any
    /// terminal side effect, while the task handle is still live.
    #[must_use]
    pub fn settle_delivery(&self, id: TaskId) -> Option<DeliveryOwnership> {
        let handle = self.tasks.get(&id).map(|h| h.clone())?;
        loop {
            let current = DeliveryOwnership::from_u8(handle.delivery.load(Ordering::Acquire));
            let terminal = match current {
                DeliveryOwnership::ForegroundWaiting => DeliveryOwnership::TerminalForeground,
                DeliveryOwnership::Background => DeliveryOwnership::TerminalBackground,
                terminal => return Some(terminal),
            };
            if handle
                .delivery
                .compare_exchange(
                    current.as_u8(),
                    terminal.as_u8(),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
            {
                return Some(terminal);
            }
        }
    }

    #[must_use]
    pub fn delivery_ownership(&self, id: TaskId) -> Option<DeliveryOwnership> {
        self.tasks
            .get(&id)
            .map(|handle| DeliveryOwnership::from_u8(handle.delivery.load(Ordering::Acquire)))
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

    /// Park until task `id` finishes, regardless of its output owner.
    /// Used by explicit inspection/resume paths, which must observe the real
    /// terminal state even for tasks that were backgrounded at spawn.
    pub async fn await_completion(&self, id: TaskId) -> Option<TaskSnapshot> {
        self.await_with_delivery(id, false).await
    }

    /// Wait for a foreground tool call, but return a running acknowledgement
    /// if the TUI hands its output ownership to the background outbox.
    pub async fn await_foreground_completion(&self, id: TaskId) -> Option<TaskSnapshot> {
        self.await_with_delivery(id, true).await
    }

    async fn await_with_delivery(
        &self,
        id: TaskId,
        acknowledge_background: bool,
    ) -> Option<TaskSnapshot> {
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
                if acknowledge_background
                    && matches!(
                        DeliveryOwnership::from_u8(handle.delivery.load(Ordering::Acquire)),
                        DeliveryOwnership::Background | DeliveryOwnership::TerminalBackground
                    )
                {
                    return Some(TaskSnapshot {
                        task_id: id,
                        status: TaskStatus::Running,
                        output: String::new(),
                        cost_usd: Decimal::ZERO,
                        finished: false,
                    });
                }
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

    /// Claim the one-shot terminal side-effect slot for task `id`.
    /// Returns `true` for the FIRST caller (which then owns publishing the
    /// terminal `SubagentSettled` / injecting the result) and `false`
    /// thereafter. A task whose handle was already finalized (removed)
    /// returns `false` — its terminal side-effect already fired. Guards
    /// ONLY the side-effect; the quota decrement in [`Self::finalize`]
    /// stays idempotent via `saturating_dec` (see Finding 13).
    #[must_use]
    pub fn claim_settle(&self, id: TaskId) -> bool {
        self.tasks
            .get(&id)
            .map(|h| h.claim_settle())
            .unwrap_or(false)
    }

    /// Wake the re-armable driver for task `id` (Phase 2 `inbox_notify`).
    /// Used by Phase 4 message delivery + Phase 3 resume to nudge a parked
    /// driver back into `run_loop`. No-op for an unknown/finalized task.
    pub fn wake(&self, id: TaskId) {
        if let Some(h) = self.tasks.get(&id) {
            h.inbox_notify.notify_waiters();
        }
    }

    /// Clone the `inbox_notify` wake handle for task `id` so the driver can
    /// park on it without holding the `DashMap` guard across an `.await`.
    /// `None` when the task is unknown / finalized.
    #[must_use]
    pub fn inbox_notify(&self, id: TaskId) -> Option<Arc<tokio::sync::Notify>> {
        self.tasks.get(&id).map(|h| h.inbox_notify.clone())
    }

    // ---- Roster (Phase 4) --------------------------------------------

    /// Register a live sibling under `root` with a UNIQUE handle name,
    /// auto-suffixing on collision (`reviewer`, `reviewer#2`, …) so two
    /// same-slug siblings are individually addressable (Finding 10).
    /// Returns the assigned [`HandleName`] and the entry's generation.
    /// `parent` scopes reachability; `allowlist` is the receiver's tool
    /// set (consulted by the sender's privilege check).
    pub fn register_name(
        &self,
        root: SessionId,
        slug: &str,
        task_id: TaskId,
        parent: SessionId,
        allowlist: Arc<[String]>,
    ) -> (HandleName, u64) {
        let generation = self.roster_gen.fetch_add(1, Ordering::AcqRel);
        let mut map = self.roster.entry(root).or_default();
        // Unique-name enforcement: first `slug` wins the bare name; later
        // same-slug siblings get `slug#N` for the smallest free N.
        let name = if map.contains_key(&HandleName(slug.to_string())) {
            let mut n = 2;
            loop {
                let candidate = HandleName(format!("{slug}#{n}"));
                if !map.contains_key(&candidate) {
                    break candidate;
                }
                n += 1;
            }
        } else {
            HandleName(slug.to_string())
        };
        map.insert(
            name.clone(),
            RosterEntry {
                task_id,
                generation,
                parent,
                allowlist,
            },
        );
        (name, generation)
    }

    /// Remove a sibling from `root`'s roster (called when the task is no
    /// longer background-alive). Idempotent.
    pub fn remove_from_roster(&self, root: SessionId, name: &HandleName) {
        if let Some(mut map) = self.roster.get_mut(&root) {
            map.remove(name);
        }
    }

    /// Resolve a handle name to its current roster entry under `root`.
    /// Returns `None` when the name is unknown / no longer addressable
    /// (the sibling finalized) — the caller surfaces a typed "not
    /// addressable" error rather than misrouting (Finding 2).
    #[must_use]
    pub fn resolve_name(&self, root: SessionId, name: &HandleName) -> Option<RosterEntry> {
        self.roster.get(&root).and_then(|m| m.get(name).cloned())
    }

    /// Snapshot the roster for `root` as `(name, task_id, gen)` triples —
    /// the data source for the `subagent.roster` SSE frame + the TUI
    /// @mention typeahead (Finding 11). Sorted by name for a stable frame.
    #[must_use]
    pub fn roster_snapshot(&self, root: SessionId) -> Vec<(HandleName, TaskId, u64)> {
        let mut out: Vec<_> = self
            .roster
            .get(&root)
            .map(|m| {
                m.iter()
                    .map(|(n, e)| (n.clone(), e.task_id, e.generation))
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by(|a, b| a.0.0.cmp(&b.0.0));
        out
    }

    // ---- Mailbox (Phase 4) -------------------------------------------

    /// Push a message onto task `id`'s inbox, enforcing BOTH the depth cap
    /// and the per-message length cap (Finding 2 — bound length, not just
    /// depth). Returns `Ok(())` on success, or a typed [`SpawnError`] when
    /// the target is unknown / over-length / the inbox is full. On success
    /// the task's re-arm `inbox_notify` is woken so a parked driver drains
    /// it (Phase 2).
    pub fn push_message(&self, id: TaskId, from: &str, body: &str) -> Result<(), SpawnError> {
        if body.len() > self.max_message_bytes {
            return Err(SpawnError::MessageRejected(format!(
                "message body {} bytes exceeds cap {}",
                body.len(),
                self.max_message_bytes
            )));
        }
        let Some(handle) = self.tasks.get(&id) else {
            return Err(SpawnError::MessageRejected(
                "target task not addressable (finalized or unknown)".into(),
            ));
        };
        {
            let mut inbox = handle.inbox.lock().unwrap();
            if inbox.len() >= self.max_inbox_depth {
                return Err(SpawnError::MessageRejected(format!(
                    "recipient inbox full ({} messages)",
                    self.max_inbox_depth
                )));
            }
            inbox.push_back(super::task_types::InboxMessage {
                from: from.to_string(),
                body: body.to_string(),
            });
        }
        handle.inbox_notify.notify_waiters();
        Ok(())
    }

    /// Drain ALL queued inbox messages for task `id` (called by the
    /// re-armable driver at the top of a loop iteration). Empty vec when
    /// the task is unknown or the inbox is empty.
    #[must_use]
    pub fn drain_inbox(&self, id: TaskId) -> Vec<super::task_types::InboxMessage> {
        let Some(handle) = self.tasks.get(&id) else {
            return Vec::new();
        };
        let mut inbox = handle.inbox.lock().unwrap();
        inbox.drain(..).collect()
    }

    /// `true` if task `id` has undrained inbox messages — the re-arm
    /// predicate for a messaging-alive subagent (Phase 4).
    #[must_use]
    pub fn inbox_nonempty(&self, id: TaskId) -> bool {
        self.tasks
            .get(&id)
            .map(|h| !h.inbox.lock().unwrap().is_empty())
            .unwrap_or(false)
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
