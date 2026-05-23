# Journal — Phase 1 Foundation

**Date:** 2026-05-23
**Phase:** 1 (Foundation)
**Status:** Complete

## What shipped

Six-crate Rust workspace, dyn-AppState axum server, six locked adapter traits, plugin API surface, `/v1/health` + Swagger UI, clap `serve|audit` CLI, env-driven `Config`, JSON tracing, graceful Ctrl+C/SIGTERM. Cross-check report at `research/cross-check-phase-01.md`.

Acceptance: build/clippy/run clean. `/v1/health` → `{"ok":true,"version":"0.1.0"}`. `/doc/openapi.json` valid 3.1 with `HealthDto`. Bind defaults to `127.0.0.1:8787`.

## Decisions confirmed during implementation

- **Edition 2024 + resolver 3** at workspace root. claw-code uses 2021/resolver 2; we upgraded.
- **`safe_failure_class()` adopted as `CoreError::class() -> FailureClass`.** Closed enum, no `Other(String)` variants — satisfies §S preemptively. Pattern stolen from claw-code, but using `thiserror` for `Display` instead of hand-rolling.
- **`broadcast_tx` removed from `AppState`** after code review (C1). Single seam for events is `events: Arc<dyn EventSink>` — Phase 5's two-tier publisher (§G) will live there. Holding the raw broadcast sender in AppState would have allowed callers to bypass persistence silently.
- **`Message::seq` removed.** Storage-assigned monotonic ids will live on the row, not on the input DTO (C4). Phase 2 SqliteMemoryStore decides the type.
- **`HookResult::Replace` is non-terminal with audit log**, distinct from `Stop` which terminates the chain. Documented explicitly in `hooks.rs` to prevent plugin-author confusion (C2).

## Divergences from references (deliberate)

- Health body: `{ok, version}` (plan-locked) vs opencode's `{healthy, version}`.
- AppState seam: `Arc<dyn _>` (per §B) vs claw-code's enum-dispatch — claw-code's approach can't accept runtime-loaded providers, which we'll need for plugin-defined providers.
- Async runtime: end-to-end async (plan §17.5) vs claw-code's sync facade + ad-hoc `block_on`.

## Unresolved questions

None blocking Phase 2. Two carry-overs:
1. Whether `Replace` should disable subsequent `Replace`s in the same chain (C2). Decision deferred until first plugin needs it.
2. Whether `permission_ruleset_path` env var name is final — config TOML support in Phase 8 may rename.

## Phase 2 setup

Phase 2 (Storage and Message Model) starts from a green workspace. SqliteMemoryStore will land first; `Message` DTO change means storage stamps `seq` on insert. `session_reads` table per §F lands alongside the schema. Pruning per §D is conceptually scoped to `project_for_llm` — landing point belongs to whichever crate hosts conversation projection (likely `openlet-core`).
