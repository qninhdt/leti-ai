# Phase 4 — Python Sandbox Executor (Monty)

**Date:** 2026-07-08
**Plan:** `plans/260708-1550-emulated-bash-python-executors/` (P4 done)
**Scope:** New `python` tool backed by an in-process Monty VM, security-by-construction like the emulated bash executor.

## What shipped

- `crates/leti-adapters/src/pyexec/` — new adapter module:
  - `executor.rs` — `MontyExecutor` (impl `PythonExecutor`). Drives `MontyRun::start`→`OsCall.resume` loop from an `async fn`; `max_memory` (256 MiB default) + `max_duration` (= timeout). Maps `Complete`→stdout+last-expr echo, exception→stderr+exit 1, `TimeoutError`→`timed_out`.
  - `mount_bridge.rs` — the single IO seam. One match arm per `OsFunctionCall` variant → `ctx.fs` async. Encodes two spike contracts: write-family resume with `Int(count)` (FIND-D), and `WriteText`=truncate / `AppendText`=append (Monty flips 2nd write on a `w` handle to `AppendText`). Curated `ENV_ALLOWLIST` (no PATH, no secrets); `FsError`→CPython exception type (`FileNotFoundError`/`PermissionError`/`OSError`).
- Registration fan-out: `CoreToolsPlugin::with_python()` builder (4-arg `new` unchanged); `all_plugins` gains a trailing `Option<Arc<dyn PythonExecutor>>`; `install_plugins` threads it. All 7 internal call sites updated (server binary passes `Some(MontyExecutor)`, every test harness passes `None`).
- Production wiring: `AdapterStack.python` (Monty), consumed by `main.rs` + `doctor_cmd.rs`.
- Toolchain already at 1.96.1 (Monty needs ≥1.95); Monty pinned by git `rev`.

## Verification

- `cargo build --workspace` clean.
- `pyexec_executor.rs` — 21 tests: compute/json/re, open()/pathlib routed through `ctx.fs`, `/etc/passwd` + `../` denied, `os.system`/`socket`/`subprocess` denied, memory-bomb + `while True` trip limits (process survives), curated env, multi-write append, error/syntax → stderr not panic.
- `registration_smoke.rs` — added `with_python_registers_the_python_tool` (14 tools) alongside the default 13.
- Full workspace `cargo test` green (130 suites). Clippy clean on `pyexec` (pre-existing collapsible-if warnings elsewhere are from the 1.88→1.96 toolchain bump, not this change).
- `code-reviewer`: APPROVE-WITH-NITS.

## Notes / follow-ups

- **Cancel vs CPU-bound guest:** `MontyRun` is synchronous between pauses, so a pure-compute loop can't be preempted by `ctx.cancel` — only `max_duration` stops it (mirrors emushell). Comment corrected to say so; not blocking.
- **Read-mode traversal error type:** `open('../x','r')` surfaces `FileNotFoundError` (via `fs.exists`→false) rather than `PermissionError`. Security-neutral (still denied); `'w'` mode reports `PermissionError` correctly.
- Real Monty file-API limits (accepted): `for line in f` (use `.read().splitlines()`), `r+` update mode. `class`/`match` unsupported (computation-only, user-accepted).

## Unresolved questions

- Should the executor use `spawn_blocking` to avoid a CPU-bound guest holding a tokio worker for up to the 120s timeout ceiling? Deferred — same trade-off exists for emushell.
