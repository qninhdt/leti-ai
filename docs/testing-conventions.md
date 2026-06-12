# Testing Conventions

Workspace-wide rules for writing integration tests across `openlet-core`,
`openlet-adapters`, and `openlet-server`. Updated as part of the
integration-test-suite phase 1 foundation.

## Stack

| Concern | Tool | Notes |
|---|---|---|
| Runner | `cargo test` | No nextest. Keep CI surface minimal. |
| Async | `tokio::test` | Use `flavor = "multi_thread"` for race tests; `start_paused = true` for time-dependent code. |
| Parametrize | `rstest` 0.23 | Replaces hand-rolled `for _ in 0..N` loops over fixture vectors. |
| Property-based | `proptest` 1.5 | For state-machine invariants (compaction, doom-guard, processor). |
| HTTP fakes | `wiremock` 0.6 | Provider-fault scenarios that exceed `MockOpenAiService`. |
| Persistence | `SqliteMemoryStore` (`:memory:`) | Real adapter where the cost is cheap. |
| Filesystem | `tempfile::TempDir` | Always — never write to fixed paths. |

## Live E2E tiers

End-to-end tiers drive a real server (real axum HTTP + SSE + provider
stream). The LLM is either the in-process `mock-openai-service` (a real
OpenAI-compat HTTP server, deterministic + network-free) or real
OpenRouter. Storage / filesystem / permission layers are always real —
never mocked — in these tiers.

| Tier | File(s) | Gate | Run |
|---|---|---|---|
| Mock-LLM (default) | `live_e2e_server_core`, `live_e2e_plugin_agent`, `live_e2e_fs_write`, `live_e2e_session_persist` | none — runs on plain `cargo test` | `cargo test --workspace` |
| Real OpenRouter (gated) | `live_e2e_openrouter_gated`, `live_e2e_fs_agent_crud`, and the other `live_e2e_*` scenario files | runtime env only: `OPENLET_LIVE_E2E=1` + `OPENROUTER_API_KEY` (no `#[ignore]`) | `OPENLET_LIVE_E2E=1 OPENROUTER_API_KEY=... cargo test -p openlet-server` |
| TUI Node wire-double (default) | `tui/tests/e2e/tui-live-e2e.test.tsx` | none | `cd tui && npm test` |
| TUI real-binary (gated) | `tui/tests/e2e/tui-real-binary-e2e.test.tsx` | `OPENLET_TUI_REAL_E2E=1` (+ `OPENLET_LIVE_E2E=1` + key for the OpenRouter sub-tier) | `cd tui && npm run test:e2e:real` |

The real-OpenRouter scenario files (`live_e2e_*`) carry **no `#[ignore]`
attribute**. They are gated purely at RUNTIME through the shared harness:
`LiveServer::for_scenario` (and its variants) use the real
`OpenRouterProvider` only when `OPENLET_LIVE_E2E=1` AND `OPENROUTER_API_KEY`
are both set. Unset (the keyless CI default), the harness transparently
falls back to the in-process scripted mock driving the SAME test body — so
`cargo test` makes no network calls and the scenarios still exercise the
full transport/sqlite/plugin wiring. There is no `cargo test -- --ignored`
step; the env vars are the single source of truth.


The TUI real-binary tier spawns the prebuilt `openlet-server` +
`mock-openai-service`; build them first (`cargo build -p openlet-server
-p openlet-test-mock-provider`). The deterministic FS proof is the
single-`write` `fs_write_once` scenario (the stateless mock can't script
multi-step CRUD); the full create→read→edit→delete sequence is proven by
the gated real-OpenRouter `live_e2e_fs_agent_crud` tier, where a real
model advances the steps. Both require the session in `danger` permission
mode (set via `POST /v1/session/:id/mode`) so `write`/`bash` auto-approve
instead of parking on an `Ask`.

## When to use what

| Need | Use | Don't use |
|---|---|---|
| Drive `ConversationRuntime` end-to-end | `common::runtime::RuntimeFixture` (core) | Hand-rolled `ConversationRuntime::new(...)` boilerplate |
| Test HTTP routes | `support::TestHarness` (server) | New parallel `common/test_app.rs` (DRY violation) |
| Provider chunk shapes | `common::wiremock_helpers::mount_*` (adapters) | Inline `wiremock::Mock::given(...)` for cases the helpers cover |
| SQLite race | Real `SqliteMemoryStore` via `common::sqlite_helper::make_pool` | `MockMemoryStore` (no monotonic seq) |
| Permission-only flow | `common::mock_permission::{AllowAll, DenyAll, ScriptedPermission}` | New permission mocks |

## Test taxonomy — layer + mock policy

Every test belongs to one layer; the file's doc comment should say which.
The rule of thumb is **mock the boundary, never the logic under test**.

| Layer | Location | Real | Mocked | Example |
|---|---|---|---|---|
| **Unit** | inline `#[cfg(test)]` | the logic under test | I/O, time, provider, stores | cost math, compaction decision, token estimate, doom guard, dispatch fault synthesis |
| **Integration** | `crates/*/tests/` | local adapters (sqlite `:memory:`, localfs, bus) | model provider only | sqlite paging, bus replay, `plugin_fault_observability` |
| **E2E (mock-LLM)** | `live_e2e_*` / `subagent_e2e` | full wiring over loopback TCP or in-proc AppState | LLM responses (scripted/`MockOpenAiService`) | session persist, fs write, `subagent_e2e` |
| **E2E (real-LLM)** | `live_e2e_*`, runtime-gated by `OPENLET_LIVE_E2E=1` + key | everything incl. the model | nothing | `live_e2e_openrouter_gated` |

- The **mock-LLM e2e layer is the default keyless CI path** — it exercises real transport/sqlite/plugins with deterministic model output.
- **Never mock a store to dodge a contract** (e.g. sqlite's monotonic `seq`): use the real local adapter — its rich suite is the cloud-impl reference (Phase 7 contract spec).
- The single permitted real-LLM layer costs money + needs a key; it stays double-gated so keyless CI is green.

## Time discipline
- **Virtual time:** prefer `#[tokio::test(start_paused = true)]` + `tokio::time::advance(...)` over real sleeps.
- **Real OS sleeps:** allowed only for OS-level guarantees (signal delivery, pgroup teardown). Document in test header why.
- **Per-test budget:** under 2 seconds on `cargo test` unless tagged `#[ignore]` with reason.

## `#[ignore]` policy

`#[ignore]` is reserved for documented blockers, not flake skipping.

```rust
#[tokio::test]
#[ignore = "blocked: clean-parent-exit pgroup leak — production fix tracked in openlet-ai#XXX"]
async fn close_pgroup_on_clean_exit() { ... }
```

A test added with `#[ignore]` MUST cite either an open issue or a
follow-up plan path. CI reports ignored count; unexplained growth fails
review.

Note: `#[ignore]` is **not** how the live-LLM tiers are gated — those use
the runtime env-var fallback described under "Live E2E tiers". Reserve
`#[ignore]` for tests that genuinely cannot run yet (a documented blocker).

## Race tests

Concurrency tests are statistical, not exhaustive. Pick iteration counts
empirically: 50 for "should never panic", 100-200 for ordering and
seq-monotonicity invariants. Use `tokio::join!` / `JoinSet` for
deterministic spawns; avoid `thread::spawn` from async tests.

`loom` and `shuttle` are not in scope for this phase. They land in a
follow-up plan.

## proptest config

Default cases per property: 64 (set via `proptest!` `#![proptest_config]`
attribute when needed). For `--release` CI lanes, override via
`PROPTEST_CASES=256`.

When proptest shrinks to a real bug:
1. File an issue with the minimal case.
2. Add a `#[test]` regression test for the minimal case.
3. Do NOT add `prop_assume!` to skip — that hides the bug.

## File organization

- Each integration test file is a separate target: `tests/<feature>.rs`.
- Shared helpers go in `tests/common/` and are imported via `mod common;`
  inside each test file. Helpers are NOT shared across crates (workspace
  dev-dep cycles).
- Fixture data lives in `tests/fixtures/<subsystem>/`.
- File names are kebab-case Rust convention: snake_case (`turn_loop_end_to_end.rs`).

## Negative tests

Every public surface a test exercises must have at least one negative
case (error path, edge value, panic boundary). Happy-path-only tests
catch ~30% of regressions; the negative cases catch the other 70%.

## Edge cases — explain the why, not the plan

Edge cases must derive from a real failure mode (race, invariant break,
boundary, attack vector). Document the *failure mode* in the test
doc-comment. Do NOT cite plan phase numbers, finding codes, or audit
labels — those rot when plans get renumbered or archived.

```rust
//! Good: explains the contract under test.
//! Tool result Part with no preceding tool_call must be dropped by
//! projection (orphan tool result would crash provider with 4xx).
```

```rust
//! Bad: cites plan artifact that may not exist later.
//! Threat model entry: phase-04 §3 — record_read race.
```

## Linking from README

The README's testing section points here. When this file changes,
update the README link/anchor.
