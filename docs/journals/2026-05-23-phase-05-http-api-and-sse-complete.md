# Phase 5: HTTP API and SSE — Complete

**Date:** 2026-05-23
**Branch:** `main`
**Plan:** [phase-05-http-api-and-sse.md](../../plans/20260523-1414-leti-agent-core-mvp/phase-05-http-api-and-sse.md)

## What shipped

The runtime is reachable over HTTP. Eleven REST routes under `/v1` cover session CRUD, fire-and-forget prompts, abort, agent listing, permission replies, plugin introspection, health, and OpenAPI docs. A single multiplexed SSE channel fans out `AgentEvent`s with durable replay-from-checkpoint via `Last-Event-ID`.

- **Routes** — `crates/leti-server/src/routes/{session,message,cancel,agent,permission,plugin,event}.rs`. Each module is `#[utoipa::path]`-annotated; `OpenApiRouter::routes!()` aggregates them in `router.rs`.
- **DTO layer** — `crates/leti-protocol/src/dto/{error,agent,session,part,message,permission,event}.rs` with `utoipa::ToSchema` + `From<DomainType>` impls so handlers stay thin. `leti-core` now derives `ToSchema` on `Role`, `SessionStatus`, `PermissionMode` so the protocol crate can re-export schemas without re-defining types.
- **BroadcastBus rewrite** — `crates/leti-adapters/src/bus/mod.rs`. Two-tier publish: `Persistence::Durable` writes to SQLite first (assigns autoincrement `event_id`) then broadcasts; `Persistence::Transient` (used for `part.delta`, `heartbeat`) broadcasts only. `replay_since(session_id, after_id)` queries the `events` table for resume.
- **DeliveredEvent envelope** — `crates/leti-core/src/adapters/event_sink.rs`. `subscribe()` now returns `broadcast::Receiver<DeliveredEvent>` carrying `(event_id, event)`; durable rows have an id, transient frames don't. SSE writes `id: <event_id>` only when present.
- **AppError → ErrorDto** — `crates/leti-server/src/error.rs`. Stable `code` slugs per HTTP status; explicit `From` impls for every domain error variant (`MemoryError`, `ArtifactError`, `EventError`, `PermissionError`, `ConfigError`, `ProviderError`, `ToolError`, `CoreError`). No catch-all 500 for known cases.
- **Fire-and-forget `prompt_async`** — appends user message + parts, marks session `Running`, spawns the runtime loop on a tokio task, returns `202 Accepted` with `{message_id, ack: true}` immediately. Errors propagate via SSE `error` events, not the HTTP response.
- **Cancel route** — `POST /v1/session/:id/abort` flips the `CancellationToken` registered in `AppState::active_turns: DashMap<SessionId, TurnHandle>` and returns `200` without awaiting teardown (§N).
- **§I crash recovery** — `main.rs` lists `Running` sessions on boot and marks them `Errored("crashed")` so they don't ghost forever after an unclean shutdown.
- **Test harness** — `crates/leti-server/tests/support.rs` wires a fully-booted router over in-memory SQLite + a stub provider that errors on every call. Eight integration tests exercise session CRUD, agent listing, permission reply 404, SSE replay, abort happy path, and empty-prompt rejection.

## Decisions worth remembering

**`Last-Event-ID` is HEADER-only.** Amendment §C ratified — no query-param alias. The header is what every spec-compliant SSE client sends on reconnect; offering both invites drift between curl smoke tests and real clients.

**Single global broadcast, server-side filter.** Capacity 1024. Slow consumers drop frames silently; the `events` table is the durable log and clients reconnect with `Last-Event-ID` to recover. Per-session broadcasts would have been a premature scale-up.

**Heartbeat at 15s, not opencode's 10s.** Cross-check phase-05 settled on 15s — long enough to be cheap, short enough to beat the typical 60s proxy idle timeout.

**Two-tier publish lives in the bus, not the call site.** `EventSink::publish(event, persistence)` takes the tier as an argument. The runtime can't accidentally durable-log a `part.delta` storm because the call site has to opt in. The bus does the right thing for each tier.

**`tokio_util::CancellationToken` over `JoinHandle::abort()`.** Token first, abort as backstop. The plan flagged that `abort()` doesn't kill subprocesses; the token routes cooperatively through every await point in the loop.

**utoipa::ToSchema on core types.** Pulled `utoipa.workspace = true` into `leti-core` so `Role`, `SessionStatus`, `PermissionMode` are first-class schemas. The alternative — wrapping each as a separate enum in protocol — would have doubled the variants and invited drift.

## Friction

**Test fixtures and `Config::default()`.** `Config` doesn't impl `Default`. The harness manually constructs every field. Worth a `Config::for_test(tempdir)` helper later, but not in this phase.

**Type-shared lib.rs.** `support.rs` initially defined its own `AgentResources` struct mirroring the server's. The router rejected it (different type). Fix was to expose `AgentResources` from `leti_server` directly so tests reach for the canonical type. Pulled `pub mod app_state` etc. up into a new `lib.rs`.

**Clippy round trip.** Build green, tests green, clippy `-D warnings` flagged four spots: `manual_implementation_of_ok` on a `match res { Ok(d) => Some(d), Err(_) => None }` filter_map (replaced with `.ok()`), two `ok_or_else` closures wrapping non-allocating values (replaced with `ok_or`), and one `or_insert_with(ReadHistory::new)` on a `Default`-implementing type (replaced with `or_default()`). Removed the now-unused `ReadHistory` import as a side effect.

## Gates passed

- `cargo build --workspace`: 0 errors
- `cargo test --workspace`: 112 passed (28 suites, 0.62s) — 8 new server integration tests
- `cargo clippy --workspace --all-targets -- -D warnings`: clean

## Deferred (need running server)

Smoke tests in the success-criteria list that require a live binary — `swagger-cli validate`, `openapi-typescript`, `curl -N` SSE smoke, manual cancel/permission/heartbeat smokes — are gated on the deploy step and tracked alongside the manual QA pass. Integration tests cover the equivalent paths end-to-end via `tower::ServiceExt::oneshot`.

## Next

Phase 6 (Ink TUI) consumes this surface. The DTO crate is the contract: `openapi-typescript` against the running server should produce a `.d.ts` the TUI imports verbatim. Phase 7 layers compaction onto the existing turn loop. Phase 8 hardens for ship.
