# Audit Bug Fixes — 3-Phase Cook

**Date**: 2026-06-01 05:28
**Severity**: Critical (concurrency + correctness)
**Component**: openlet-core, openlet-adapters, openlet-server, core-agents
**Status**: Resolved (14/14 fixes shipped, 528/528 tests green, clippy clean)
**Plan**: `plans/20260601-0346-bug-fixes-audit/`

## What Happened

The bug-fix half of the 2026-05-31 three-agent audit, split from the refactor/test sibling
plan so correctness ships first as a small, low-risk, review-cheap unit. 14 fixes:
3 critical, 5 high, 4 medium, 2 low. Cooked end-to-end in `--auto`. Every phase carried the
2026-06-01 red-team corrections folded into the plan.

## The Brutal Truth

Two of the "critical/high" fixes were traps the red team had already defused, and the plan
encoded the defusing — so the discipline was to NOT "fix" already-correct code:

- **C2** (orphaned permission rule): the original plan said "push in-memory first, then
  persist, rollback on failure" — which REVERSES already-correct persist-first code and opens
  a TOCTOU privilege-escalation. Verified `accept_ask` persists-first, `hydrate` replays on
  boot, recovery already guaranteed. Net code change: zero. Net test added: one documenting
  regression. Resisting the urge to "implement" was the fix.
- **H5** (lost reasoning on flush error): not a separate change — folded into C3's
  validate-all-then-drain. One function, one coherent pass, two findings closed.

**H1 was the real work and the real risk.** The plan's soft goal ("reduce lock contention")
directly conflicts with its hard goal ("zero dropped events"). Doing INSERTs outside the lock
with concurrent commits reintroduces the exact replay-seam hole the mutex prevents — a
reconnecting SSE subscriber's DB replay can observe id N+2 before N+1 is durable. Chose
correctness: allocate→persist→broadcast stay under one lock (SQLite serializes writers anyway,
so the contention cost is marginal). Added the MANDATORY `MAX(id)` seed so the explicit-PK
counter doesn't collide with surviving rows on restart.

## Technical Details — Fix Highlights

| Fix | Win | Severity |
|---|---|---|
| C1 | `await_completion` reads snapshot from the already-cloned handle — closes "task vanished" race without touching `finalize` (sole quota release) | Critical |
| C2 | Verify-only: persist-first order confirmed unchanged; `hydrate` replays orphan on boot; documenting test | Critical |
| C3 | `flush_into_parts` → validate-all-then-drain; dup `call_id` across indices = clean Decode; reasoning/text preserved on error (also satisfies H5) | Critical |
| H1 | event_id allocated+persisted+broadcast under one lock, seeded from `MAX(id)`; new `append_with_id`; counter self-heals on append error | High |
| H3 | `root_session_of` returns `Result` — DB errors propagate as `SpawnError::Internal`, no silent wrong-root quota corruption | High |
| H4 | `LoopOutcome::final_assistant_message_id` → `Option<MessageId>`; nil-UUID sentinel removed at all 5 constructors; consumer skips `list_parts` on `None` | High |
| H6 | `await_reply` drains already-delivered answer via `try_recv` before the still-`biased` select — honors a given answer without weakening consent revocation | High |
| M3 | `tracing::error!` before synthetic "dispatch slot lost" | Medium |
| M4 | `AgentDefinition::validate()` rejects threshold ∉ (0.0,1.0]/NaN at load; `should_compact` skips+logs on NaN/non-positive | Medium |
| M6 | `pending_per_session` → set (`DashMap<SessionId,()>`); `release_session_slot`→`remove_session_slot` | Medium |
| M7 | duplicate inherent `peek_session_id` removed; trait method sole def | Medium |
| L2 | cache-write cost: `saturating_add` → `max()` (no double-charge if both fields set) | Low |
| H2-moved | post-compaction `should_compact` threads the shared provider-actual anchor instead of hardcoded `None` | Low |

## Review & Hardening

Adversarial code review: **APPROVE 9/10**. No criterion unmet, no touchpoint regression, no
unintended contract break. Three LOW latent hazards flagged — none reachable on current paths
— all hardened anyway:

1. **H1 cancellation edge**: if `publish()` is dropped mid-`append_with_id`, the cached counter
   could wedge into a permanent UNIQUE-collision. Now resets to `None` on append error → next
   publish re-seeds from `MAX(id)`. Documented `publish()` as cancel-to-completion.
2. **M4 negative clamp**: a negative threshold clamped to `0.0` → `limit=0` → compact-every-turn.
   Now skips+logs on non-positive, same as NaN. Never-compact is the safe failure mode.
3. **Dead `append`**: the old AUTOINCREMENT method is now caller-less; marked `#[deprecated]`
   to prevent bus-counter drift if reused.

## Intentional Public-Contract Changes (called out)

- `LoopOutcome::final_assistant_message_id`: `MessageId` → `Option<MessageId>` (H4)
- `QuestionRegistry::release_session_slot` → `remove_session_slot`; map value `u8` → `()` (M6)
- `SqliteEventRepo`: `append_with_id` + `max_event_id` added; `append` deprecated (H1)
- `ConfigPermissionMgr` inherent `peek_session_id` removed; trait method sole def (M7)
- `AgentDefinition::validate()` added (M4)

All callers updated; full suite green.

## Unresolved Questions

- `hydrate` re-scopes recovered Global rules as `Session`-scoped (`manager.rs`). Pre-existing,
  NOT introduced here, surfaced during C2 review. Belongs to the sibling refactor/test plan if
  pursued — out of scope for this correctness-only unit.
