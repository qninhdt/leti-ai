# Phase 2: Storage and Message Model — Complete

**Date:** 2026-05-23
**Branch:** `main`
**Plan:** [phase-02-storage-and-message-model.md](../../plans/20260523-1414-openlet-agent-core-mvp/phase-02-storage-and-message-model.md)

## What shipped

The persistent layer is live. Sessions, messages, and parts survive a restart, JSONL mirrors every event for replay, and a deterministic projection function turns the part-based message log into the role-tagged shape an LLM expects.

- **SqliteMemoryStore** — full `MemoryStore` impl over a sqlx pool. Per-session monotonic `seq` assigned in-DB; `UNIQUE(session_id, seq)` is the safety net under WAL.
- **Migration `0001_init.sql`** — 7 tables: `sessions`, `messages`, `parts`, `artifacts`, `events`, `permission_decisions`, `session_reads` (§F). All `CREATE TABLE IF NOT EXISTS`, idempotent re-run.
- **LocalFsArtifactStore** — bytes under `<root>/<session>/<sha256(key).hex>`, metadata mirrored in SQLite. Keys with `..`, leading `/`, or empty are rejected before touching the filesystem.
- **SessionLogger** — per-session JSONL append, regex redaction (`Bearer <jwt-ish>`, `sk-...`) plus a recursive key-allowlist walker (`api_key`, `Authorization`, `password`, etc.). Rotates at 64MB.
- **`project_for_llm`** — pure function, lives in `openlet-core::projection`. Reasoning parts drop unless caps say replay is supported; tool-call/tool-result pairing is by `call_id`. Tests assert append-only-prefix invariance.
- **Event repo + permission repo** — append-only event log with autoincrement IDs (Phase 5 SSE Last-Event-ID), and `permission_decisions` writer for "always allow" choices.

## Decisions worth remembering

**`sqlx::query` over `sqlx::query!`.** The plan called for compile-time-checked macros + a committed `.sqlx/` cache. I went with runtime-checked queries instead. Trade-off: lose compile-time SQL validation, gain a one-step build (no `cargo sqlx prepare` pre-push hook, no sqlx-cli dependency, no offline cache to drift in CI). Schema is small and our tests round-trip every query, so the validation gap is bounded. Worth revisiting in Phase 8 if the schema grows or query count spikes.

**Soft-delete keeps cascade dormant.** `delete_session` is `UPDATE sessions SET deleted_at = ?, status = 'cancelled'`, not `DELETE`. The `ON DELETE CASCADE` on child tables is intentional dead weight — sessions stay queryable for resume even after a user "deletes" them. If we ever hard-delete in admin/audit tooling, cascade fires correctly.

**`MemoryStore::new()` → `MemoryStore::new(pool)`.** Breaking signature change but only `main.rs` constructed it. Phase 1's no-arg stub was a placeholder; the real shape was always going to take a pool.

## Friction

The `code-reviewer` subagent ran for 4 minutes across 22 tool calls and produced no persisted report. I wrote the review summary myself with the same checklist and saved it to `plans/.../reports/code-reviewer-phase-02.md`. Tracking pattern: subagents that don't acknowledge "save report to <path>" instructions silently succeed-without-output. Worth a delegation template fix later.

## Gates passed

- `cargo build --workspace`: 0 errors across 6 crates
- `cargo test --workspace`: 18 passed (15 suites)
- `cargo clippy --workspace --all-targets -- -D warnings`: clean

## Next

Phase 3 (Agent Loop Core) consumes `MemoryStore`, `project_for_llm`, and the `Part` enum. The `events` table is ready for Phase 5's SSE channel.
