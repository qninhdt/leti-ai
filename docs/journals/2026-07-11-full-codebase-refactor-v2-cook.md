# Full Codebase Refactor v2 — Post-Executor Drift Sweep (cook)

Date: 2026-07-11
Plan: `plans/260711-0828-full-codebase-refactor-v2` — all 10 phases
Branch: `feat/emulated-bash-python-executors`
Constraint: behavior-preserving. Green build + green tests before/after each phase.

## Goal

Targeted whole-codebase sweep of drift/new surface added since the 2026-06-12
refactor — mainly the emushell/pyexec/cloudfs adapters from the executor plan,
cross-crate DTO duplication, dead code, and stale infra/docs. No feature work,
no wire-contract changes.

## What shipped, by phase

- **P1 Baseline:** fmt/clippy/test/tui all green → safe to proceed.
- **P2 Dead code:** deleted untracked `crates/spike-executors/` (carried a
  committed `target/`); removed dead `GeneratedFileServiceClient` re-export,
  TUI `ErrorDto`, un-exported two internal reducers. KEPT + documented the
  ambiguous cloud-extension-point items (`config_perm::peek_request`,
  `AppStateBuilder::read_histories`/`active_turns`) per Session-1 decision.
- **P3 Core boundary:** moved `QuestionId` → `types/question.rs` (killed the
  only `types → runtime` edge), re-exported for back-compat. Collapsed
  `NotificationLevel` to ONE canonical def in `types/event.rs`; `hooks::io`
  re-exports it. Serde attrs kept byte-identical.
- **P4 Runtime dedup:** `PermissionRequest::simple` (14 builtin sites),
  `LlmMessage::simple`/`::tool` (projection + compaction), shared `ProcessOutput`
  aliased by Bash/PythonOutput, generic `run_denyable_chain` (both chat-hook
  fns), `for_each_chain!` macro drives `HookChains::merge` + `sort_all` from ONE
  list, `record_tool_metric` helper, split `dispatcher.rs` → `dispatcher/{mod,execute}.rs`.
  Deleted dead `ReadHistory::snapshot`, sync `poll`, `handle`.
  - **Caught a real bug mid-refactor:** initial `LlmMessage::tool(body, call_id)`
    had args swapped vs the helper's `(call_id, content)` signature — would have
    swapped tool-result body/id on the wire. Fixed before build.
- **P5 Adapters (highest value):** one `util::floor_char_boundary` replaces 5
  copies; emushell `gather`/`fs_err_msg`/`has_glob_meta` de-duped; split
  `sed_awk.rs`→`sed.rs`+`awk.rs`, `eval.rs`→`eval/{mod,expand}.rs`; split
  `cloudfs/mod.rs` 702→432 into `resolve.rs`+`convert.rs`; recovered poisoned
  `session_dirty` mutex (`.unwrap_or_else(into_inner)` vs `.expect()`). Kept the
  two cloudfs BFS walks separate (they bound MAX_FOLDERS against different sets).
- **P6 Server:** hoisted the twice-defined `ExitGuard` + slot scaffolding into
  `turn_slot.rs` (`try_claim_turn_slot`, `spawn_driven_turn`); shared
  `runtime_handles()` assembler for both turn drivers; extracted
  `boot::{recover_stale_running_sessions,assert_bind_safe}` from main.rs; fixed
  `/plugin/:id/health` emitting a durable `AgentEvent::Error` on a plain 404;
  corrected `/v1/sessions/` → `/v1/session/` doc drift.
  - Deliberately KEPT `map_perm_err` (switching to `?` would change the
    `ask_expired` wire code — not behavior-preserving) and skipped the
    append-user-message helper (3 sites have divergent SSE semantics).
- **P7 (highest risk) — Option (b), triggered by the plan's own gate:** the
  Session-1 pick was Option (a) (delete the DTO mirror, feature-gate `ToSchema`
  onto core), with a mandatory abort-to-(b) if the OpenAPI diff is non-empty.
  Found analytically that the diff MUST be non-empty: `UsageDto` is lossy (5
  fields vs core `Usage`'s 7 — it sums the two cache-token fields and drops
  `cost_usd`), and `PermissionAsked` folds `ask_id` into `PermissionRequestDto`.
  So kept the mirror + made the existing wildcard-free `From<AgentEvent>` match
  an explicit, documented compile-time exhaustiveness guard. Plugin hook API:
  one `for_each_hook_kind!` list now drives the struct fields, `new()` init,
  `into_registrations`, AND the 15 `on_*` methods — a new hook is a one-line edit.
- **P8 TUI:** split `prompt-editor.tsx` 325→192 (+3 extracted units) and
  `store/index.ts` 287→94 (pure `apply-event.ts` reducer); rewrote README
  Layout/Testing/Architecture to the real Solid/OpenTUI/Bun tree (phantom
  `status-bar.tsx`/`markdown-renderer.tsx`/`tests/e2e/` refs gone); PERF.md gate
  re-pointed at the real `render/event-pump.ts` throttle; struck the abandoned
  openapi-fetch migration promises; relabeled `schema.d.ts` as the drift snapshot.
- **P9 Infra:** Dockerfile base `rust:1.88-slim` → `rust:1.96.1-slim` (was below
  MSRV 1.95 → image never built); dropped unused OpenSSL deps; composite
  `.github/actions/rust-setup` reused by all 3 Rust CI jobs; `contract-drift`
  uses `npx --yes` instead of a 2nd full `npm ci`; fixed the truncated
  `image.yml` push cache; `server-mock` uses `extends:` instead of a duplicated
  build block; added `tui/.npmrc` (`legacy-peer-deps=true`).
- **P10 Verification:** full pre-PR gate green. `docker build` succeeds.

## Pre-existing latent issues surfaced (NOT introduced here)

1. **Docker build had TWO breakers, not one.** The plan flagged the MSRV pin.
   The `docker build` in P10 also revealed the builder stage never installed
   `protoc` / `libprotobuf-dev` — `openlet-adapters/build.rs` needs them to
   compile the cloudfs proto (added by the executor plan). The host builds only
   because it has system protoc. Fixed the Dockerfile (protobuf-compiler +
   libprotobuf-dev). The image had not built since cloudfs landed.
2. **`cargo deny`/`cargo audit` report 5 vulns + 4 unmaintained.** `Cargo.lock`
   is byte-unchanged and this refactor added zero deps, so all are pre-existing
   (anyhow, crossbeam-epoch, lopdf, pyo3×2, quinn-proto, atomic-polyfill,
   encoding, rustls-pemfile). Phase 1's baseline didn't run deny/audit, so they
   first surface here but predate the plan. Left for a dedicated dependency-bump.

## Verification

- `cargo fmt --check` · `cargo clippy --workspace --all-targets -- -D warnings` · clean
- `cargo test --workspace` · all pass
- `cd tui && npm run typecheck && npm test && npm pack --dry-run` · 86 tests pass
- `docker build .` · succeeds on pinned 1.96.1
- mock-provider parity e2e (debug/fix/verify, compaction continuity, cost/usage) · pass
- grep sweeps: no spike-executors, no phantom TUI docs, `ExitGuard`/`NotificationLevel`
  single-defined, no duplicated `PermissionRequest`/`LlmMessage` literals

## Unresolved

- Pre-existing `cargo deny`/`audit` advisories (see above) — need a separate
  dependency-bump pass; out of scope for a behavior-preserving refactor.
- `ProcessorState`/`PendingToolCall` serde derives kept (possibly reserved for a
  session-resume feature) — the P4 open question was left conservative.
