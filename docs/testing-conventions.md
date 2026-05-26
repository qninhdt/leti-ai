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

## When to use what

| Need | Use | Don't use |
|---|---|---|
| Drive `ConversationRuntime` end-to-end | `common::runtime::RuntimeFixture` (core) | Hand-rolled `ConversationRuntime::new(...)` boilerplate |
| Test HTTP routes | `support::TestHarness` (server) | New parallel `common/test_app.rs` (DRY violation) |
| Provider chunk shapes | `common::wiremock_helpers::mount_*` (adapters) | Inline `wiremock::Mock::given(...)` for cases the helpers cover |
| SQLite race | Real `SqliteMemoryStore` via `common::sqlite_helper::make_pool` | `MockMemoryStore` (no monotonic seq) |
| Permission-only flow | `common::mock_permission::{AllowAll, DenyAll, ScriptedPermission}` | New permission mocks |

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
