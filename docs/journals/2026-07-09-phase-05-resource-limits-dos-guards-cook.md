# Phase 5 — Resource Limits & DoS Guards (cook)

**Date:** 2026-07-09
**Plan:** `plans/260708-1550-emulated-bash-python-executors`
**Scope:** close the remaining in-process DoS gap for the emulated bash shell; confirm Python guards already landed in P4.

## Context

After P2 (no subprocess) + P4 (Monty), exec/network/escape are closed by
construction. The only remaining attack surface is **CPU/memory DoS
in-process**. Most of P5's plan was already satisfied early:

- bash step-budget (`MAX_STEPS`) + `ctx.cancel` checks + async-native eval → P2
- Python `max_memory` / `max_duration` + `ResourceError`→`PythonOutput` → P4
  (tests `infinite_loop_trips_timeout`, `memory_bomb_trips_limit`)
- `Option<Arc<dyn PythonExecutor>>` constructor fan-out → P4

## The real gap

The bash executor **ignored `timeout_ms`**. A pure-CPU loop
(`while true; do :; done`) touches `ctx.fs` zero times, so nothing in it
`.await`s — a wrapping `tokio::time::timeout` can never pre-empt it because
the future never yields. Only the 5M step-budget stopped it, after
monopolising a worker thread.

## Changes

- **`Interp::with_timeout(Duration)`** — absolute `Instant` deadline, checked
  in-band from `tick()` at every evaluated node. Bounds pure-CPU loops in real
  time regardless of `.await`. `checked_add` guards overflow; `timeout=0` = no
  deadline (degenerate-input safety).
- **Cooperative yield** — `run_while` calls `tokio::task::yield_now()` every
  `YIELD_EVERY` iterations so the runtime can observe cancellation and service
  co-scheduled tasks while the deadline still bounds total wall-clock.
- **`AbortReason::Timeout`** (distinct from `StepBudget`/`Cancelled`). Executor
  appends a guard-naming stderr line (`wall-clock timeout` vs `step budget
  exceeded`), maps both → `timed_out=true`, exit `-1`. Cancel keeps the
  `Err(ToolError::Timeout)` contract matching the old subprocess path.
- **cmdsub inherits the absolute deadline** → `$(while true; do :; done)` is
  bounded by the shared cutoff, not granted a fresh budget.

Python needed nothing: `max_memory`/`max_duration` are wired in P4, and the
Monty VM is owned by `start`/`resume` so an `Err` return drops it (natural
discard). `MountTable write_bytes_limit` does not apply — we use the `OsCall`
seam, not `MountTable`.

## Verification

- `cargo build --workspace` clean.
- New `emushell_dos_guards.rs`: 3 tests (wall-clock bound, cancel mid-loop,
  finite-workload-not-cut) — all pass.
- Full workspace suite green (0 failures).
- code-reviewer: APPROVE-WITH-NITS. Confirmed correct: unconditional per-iter
  `tick()`, absolute-`Instant` cmdsub inheritance, monotonic abort state,
  step-budget composition across subshells, faithful cancel/timeout mapping.
- Nits addressed: `checked_add` in `with_timeout`; de-coupled the timing-
  sensitive test to assert `timed_out` + accept either guard's stderr (avoids
  a release-build race where step-budget could win before the 500ms deadline).

## Unresolved / follow-ups

- Bash memory is still only indirectly bounded — the `for`-loop `expanded` Vec
  and pipeline intermediate `stdin` buffers are uncapped. CPU is well-guarded
  now; large-fanout memory pressure is a candidate follow-up (out of P5 scope).
- Budget/timeout thresholds are compile-time constants; no env config surface
  was added (plan mentioned it as optional). Tune at dogfood if needed.

**Next:** Phase 6 (cloud Filesystem gRPC + pg_trgm, cross-repo to `~/projects/openlet`).
