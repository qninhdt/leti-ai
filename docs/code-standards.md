# Code Standards

_Last updated: 2026-06-11_

Standards the openlet-ai codebase actually follows. Derived from the code +
`.claude/rules/development-rules.md`. Enforced by CI (`.github/workflows/ci.yml`).

## Principles

- **YAGNI / KISS / DRY.** Widen a trait only when a concrete need demonstrates
  the current shape blocks it; don't design for hypothetical futures.
- **Hexagonal.** Domain logic in `openlet-core` is IO-free; all IO lives behind
  a port trait implemented in `openlet-adapters`. Core never depends on a
  concrete backend or on `openlet-server`.
- **Seams over forks.** Extension happens through traits + the plugin API, not
  by editing core. Even the eight built-in tools ship as the `core-tools`
  plugin — proof the public surface is sufficient.

## Rust conventions

- **Edition 2024**, toolchain pinned in `rust-toolchain.toml` (stable).
- `unsafe_code = "forbid"` workspace-wide. `unused_must_use = "deny"`.
- Clippy: `all` warns; CI runs `clippy --workspace --all-targets -- -D warnings`
  (warnings fail the build). `pedantic` is allowed (off).
- `cargo fmt` is authoritative; CI runs `--check`.
- Errors: `thiserror` enums per layer (`CoreError`, `MemoryError`,
  `ProviderError`, `ArtifactError`, …). `anyhow` only at the binary boundary.
- Async: `async_trait` for the port traits; `tokio` runtime.
- Trait widening is **additive with default methods** where possible — a new
  method with a default impl leaves existing implementors (incl. test doubles)
  compiling, so only the production impl overrides. A genuine signature change
  is **atomic**: trait + all impls land in one commit, green verified post-batch.

## Naming

- Files: kebab-case, descriptive, length is fine if it aids LLM tooling
  (`turn_loop_compaction.rs`, `subagent_driver.rs`). Rust modules use
  snake_case per the language convention.
- Keep code files focused; consider splitting past ~200 lines into a sibling
  module (the runtime is split this way: `turn_loop`, `turn_loop_compaction`,
  `conversation`, `processor`, …).

## Comments

- Explain the **why**, not the what: invariants, races, trade-offs, the reason
  a non-obvious choice was made.
- **Never reference plan artifacts** (phase numbers, finding codes like F13/M16,
  audit labels) in code, comments, test names, or migration filenames — those
  headers get renumbered and the reference rots. Allowed: stable external IDs
  (RUSTSEC, SQLSTATE, CVE), and same-codebase symbol names. (See
  `.claude/rules/review-audit-self-decision.md`.)
- Commit messages describe the change, not the finding code; conventional
  commit prefixes (`feat`/`fix`/`refactor`/`test`/`docs`/`ci`).

## Testing

- Layered: unit (inline `#[cfg(test)]`) / integration (`crates/*/tests/`, real
  local adapters) / e2e mock-LLM (`live_e2e_*`, default keyless CI) / e2e
  real-LLM (`#[ignore]` + double-gated). See `docs/testing-conventions.md`.
- Mock the boundary, never the logic under test. Never mock a store to dodge a
  contract (use the real local adapter — its suite is the cloud-impl reference).
- Assertions are behavioral/invariant-based; real-LLM tests assert shape, not
  exact text. No tautological or constant-mirroring assertions.

## Security

- Secrets in `secrecy::SecretString`; never logged. The provider key is
  `set_sensitive(true)`. Real-LLM transcripts pass the evidence scrubber before
  any write.
- Reserved HTTP headers (`authorization`, `x-api-key`, …) are filtered before a
  plugin-supplied header merge.
- Zero-trust inbound: identity comes only from the `Authenticator` output,
  never an upstream-injected header.
- Workspace ids are path-traversal-validated (`workspace_data_root`).

## Pre-PR checklist (mirrored by CI)

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check && cargo audit
( cd tui && npm run typecheck && npm test && npm pack --dry-run )
```
