# Contributing

## Pre-PR checklist

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
cargo audit
( cd tui && npm run typecheck && npm test && npm pack --dry-run )
```

All six must pass before opening a PR. CI mirrors them on every PR via
`.github/workflows/ci.yml` (rust fmt/clippy/test/deny/audit + TUI
typecheck/test/pack + an OpenAPI→types contract-drift guard).

The engine boundary is intentionally strict: keep authentication, tenancy,
cloud adapters, and business policy in the embedding host. Core-facing
request metadata must use the opaque `TurnExtensions` carrier; do not add
identity semantics or persistence dependencies to `leti-core`.

## Code style

- Rust: 2024 edition, formatted by `rustfmt`, lints in
  `.clippy.toml` enforced with `-D warnings`.
- TypeScript: keep the TUI lints clean and tests green.
- File names: kebab-case for everything except Rust (snake_case) and
  Java/Swift/etc (their own conventions).

## Adding a dependency

- Run `cargo deny check` after adding. New licenses need to land in
  `deny.toml` with a one-line rationale.
- Prefer workspace deps (`<crate>.workspace = true`) over per-crate
  pins so versions stay aligned.
- Avoid wildcard versions in published crates.

## Errors and telemetry

Every error type carries a closed-set `FailureClass` (see
`leti-core::error`). New variants extend the enum; no
`Other(String)` escape hatch. Surface the slug at the
`tracing::error!(class = …, …)` call site so dashboards can group.

## Commits

Conventional commits, no AI references. Keep commits scoped to one
concern. The plan amendments (`amendments-after-red-team.md`,
`amendments-plugin-system.md`) override individual phase files on
conflict — note overrides in the commit body when relevant.

## Bug reports

Include the `class` from the error envelope and a redacted audit dump
(`leti-server audit --session-id <ID>`).
