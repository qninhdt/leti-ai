# Phase 7 — Tests, Parity & Cutover (cook)

Date: 2026-07-09
Plan: `plans/260708-1550-emulated-bash-python-executors` (final phase)

## What shipped

Cutover default was ALREADY live from prior phases (`adapter_stack.rs` wired
`EmulatedShellExecutor`+`MontyExecutor`; `main.rs`/`doctor_cmd.rs` pass
`Some(python)`). Phase 7 closed the remaining gaps + added the rollback lever.

- **Runtime rollback flag** (`adapter_stack.rs`): `ShellImpl` enum +
  `parse_shell_impl` (string→impl) + `resolve_shell_impl` (folds parse + the
  subprocess/cloud-incompat guard into one pure fn). `OPENLET_SHELL_IMPL`:
  `emulated` default (incl unset/empty), `subprocess` fallback. Unknown value →
  hard error; non-UTF-8 → hard error (`var_os`+`to_str`, not `var().ok()`).
  `subprocess`+`cloud_fs` → hard error (LocalShellExecutor bypasses `ctx.fs`).
  `LocalShellExecutor` kept behind the flag (NOT `#[cfg(test)]`) ≥1 release.
- **Security:** `/dev/tcp` egress-block test → `emushell_interpreter.rs`.
- **Interrupt-safety:** mv/cp cancel-mid-run tests → `emushell_builtins.rs`. A
  pre-tripped cancel token halts the run via the interpreter's in-band check
  (`Err(ToolError::Timeout)`) BEFORE any builtin mutates — sources intact, no
  partial dest, no rename limbo.
- **FS-impl parity** (`emushell_parity.rs`, 13 tests): `MemFilesystem`
  (`tests/common/mem_fs.rs`) — in-memory HashMap with object-store implicit-dir
  semantics, structurally unrelated to `tokio::fs`. glob/grep reuse the SAME
  `globset`/`regex` crates as `LocalFilesystem` so the pattern dialect is
  identical by construction. Asserts stdout+exit+on-disk-effect equal across
  Local vs Mem for bash pipelines/glob/for-loop/grep + python read/write. This
  is the load-bearing proof of the plan thesis (executor depends on the FS seam
  alone, not disk).
- **Live-e2e rewrite:** `live_e2e_debug_fix_verify.rs` moved from `python3`-via
  -subprocess-bash to an inline `python`-tool debug loop; `MontyExecutor` wired
  into `live_support.rs` `all_plugins(...)` + a `minimal_tool_ctx()` helper for
  the fresh-executor re-run verification.

## Factual deltas from the plan (corrected, not papered over)

1. **Monty (pinned rev) DOES support basic `class`** — `__init__`, methods,
   attributes all run (exit 0). Only `class B(A)` inheritance and `match`/`case`
   fail loud (non-zero + traceback). The plan's "class out of scope" note is
   stale. Tests now pin ACTUAL behavior:
   `basic_class_is_supported_by_pinned_monty`,
   `class_inheritance_fails_loud_not_silent`, `match_statement_fails_loud_not_silent`.
2. **Monty cannot `exec()` a file** — a nested `open()` OsCall inside `exec()`
   isn't resumable (`RuntimeError: unexpected external-call pause`). The debug
   loop is therefore INLINE code (which is exactly the plan's wording: "LLM
   writes buggy code → python runs → reads traceback → fixes → runs again"),
   not the file+exec pattern carried over from the old bash version. Inline code
   + plain `open().read()` work fine.

## Review

`code-reviewer` → APPROVE 8/10, no blockers. Deltas judged handled honestly.
Addressed its MEDIUM findings before finalize:
- Rollback "verify" was only proven at the parser level → extracted
  `resolve_shell_impl` and added wiring/guard tests (8 unit tests total).
- mv/cp interrupt-safety was untested → added cancel-mid-run tests.
- `MemFilesystem::grep` gitignore divergence from local → documented + parity
  header claim tightened + on-disk-effect parity test added.
- non-UTF-8 env value silently defaulting → now a hard error.

## Verification

- `cargo test --workspace`: 816 pass, 0 fail. No regression (MontyExecutor
  added to live_support catalog is inert — other live_e2e tests use
  non-exhaustive `contains` checks, confirmed).
- clippy: new/touched files clean. Remaining warnings are pre-existing
  toolchain-bump collapsible-if / is_multiple_of, not mine.
- `cargo deny check`: advisories + sources FAIL are PRE-EXISTING (anyhow
  RUSTSEC-2026-0190, atomic-polyfill unmaintained, Monty git-`rev` source that
  the plan itself mandates); licenses + bans OK.

## Scope decisions

- DRY: augmented existing `emushell_*`/`pyexec_*` suites instead of creating the
  duplicate `emulated_*` files the plan's file list named. Same coverage, no
  duplication.

## Unresolved / follow-ups

- Cloud-real parity still needs Phase 6's gated live e2e; the mock proves
  FS-impl independence but not the actual gRPC backend.
- `OPENLET_SHELL_IMPL` is documented in-source but has no operator-facing docs
  entry yet (reviewer NIT) — candidate for `docs/deployment-guide.md`.
- Nothing committed (ask-first); repo has uncommitted Phase 6 + Phase 7 work.
