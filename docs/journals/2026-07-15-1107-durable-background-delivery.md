---
date: 2026-07-15
topic: durable-background-delivery
---

# Durable Background Delivery

## Context

Phase 5 background task completion persistence had a crash window: the outbox
was marked `scheduled` after enqueue rather than after the parent turn actually
processed the result. A server crash in between could suppress recovery and
lose the notification.

## What happened

- Replaced `scheduled`-as-delivered semantics with a durable outbox lifecycle:
  `pending -> leased -> delivered`.
- Claims use a unique, per-claim lease token and conditional SQLite updates, so
  only the current owner can acknowledge or release a delivery. Expired leases
  are reclaimable on recovery.
- Started a per-turn heartbeat when the delivery is enqueued, retaining the
  lease while it waits in or runs through the parent turn queue. Completion
  acknowledges; error/cancellation releases; crash or panic leaves the lease
  to expire for replay.
- Contained turn-driver panics in the per-session turn slot so finalization
  releases the slot and queued work can continue.
- Added migration `0013_background_task_delivery_leases.sql`. Legacy rows with
  `scheduled_at` are intentionally replayed because the prior schema cannot
  prove that their in-memory parent turn completed.

## Decisions

This adopts the standard at-least-once plus idempotency model, rather than
trying to infer success from queue admission. The durable `(parent_session_id,
task_id)` outbox key remains the convergence point; leases make ownership and
crash recovery explicit.

## Validation

- `cargo fmt --check` passed.
- TUI typecheck, 114 TUI tests, and package dry-run passed.
- Focused Rust tests, including SQLite lease reclaim/concurrent settlement
  coverage, passed.

## Next

- Complete remaining Phase 5 full-workspace validation and review.
- Keep a focused end-to-end assertion for crash/restart delivery recovery as
  part of the phase acceptance suite.
