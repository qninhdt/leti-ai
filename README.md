# Openlet AI

Standalone Rust agent runtime exposing REST + SSE, paired with an Ink/React TUI client.

> **Status:** MVP. Eight phases complete: workspace foundation, SQLite-backed message model, sync agent loop, six-trait adapter surface (filesystem, shell, sqlite, localfs, openai-compat, broadcast bus), HTTP API + SSE, Ink TUI, compaction, and Phase-8 hardening (parity harness, audit subcommand, deny/audit clean).

## Quickstart

One command boots the server, waits for health, builds the TUI if needed,
and drops you into the live terminal app — then tears the server down on exit.

```bash
cp .env.example .env   # then fill OPENROUTER_API_KEY
./openlet-ai           # real OpenRouter
```

No key, no network? The in-process mock LLM runs the full pipeline:

```bash
./openlet-ai --mock    # network-free; no key required
```

Other flags: `--clean` (kill a straggler on the bind port + clear run state),
`--rebuild` (force a TUI rebuild), `RELEASE=1 ./openlet-ai` (release profile).

### Manual / advanced

If you'd rather drive each piece by hand:

```bash
# 1. Build everything (~90s clean).
cargo build --workspace

# 2. Run the server (defaults to 127.0.0.1:8787).
OPENROUTER_API_KEY=sk-... cargo run -p openlet-server

# 3. In another terminal, install the TUI and connect.
cd tui && npm install && npm run build && npm install -g .
openlet
```

For a network-free dry run, point the server at the in-process mock provider.
`OPENLET_MODEL_BASE_URL` is honored on the serve path (no source edit needed):

```bash
cargo run -p openlet-test-mock-provider --bin mock-openai-service
# then start the server with OPENLET_MODEL_BASE_URL=<printed base_url, verbatim>
# (the printed URL already ends in /v1 — do not append another)
```

## Configuration

All config is environment-driven. See [`docs/deployment.md`](docs/deployment.md) for the full surface.

| Env var | Default | Purpose |
|---|---|---|
| `OPENLET_BIND` | `127.0.0.1:8787` | TCP bind address. Loopback-only by default. |
| `OPENLET_DATA_DIR` | `~/.openlet` | Sqlite, artifact, and session-log root. |
| `OPENROUTER_API_KEY` | _(unset)_ | OpenRouter / OpenAI-compat credentials. |
| `OPENLET_MODEL_BASE_URL` | `https://openrouter.ai/api/v1` | Model API base URL. Point at a self-hosted gateway or the in-process mock (`./openlet-ai --mock` sets it). |
| `OPENLET_DEFAULT_MODEL` | `anthropic/claude-sonnet-4-6` | Default chat model. |
| `OPENLET_LOG_FORMAT` | `json` | `json` or `pretty`. |
| `OPENLET_ENABLE_DOCS` | `1` | Set `0` to remove the `/doc` Swagger UI in cloud builds. |
| `OPENLET_ALLOW_NON_LOOPBACK` | _(unset)_ | Set `1` to permit non-loopback bind. Required when an authenticating reverse-proxy fronts the listener. |
| `RUST_LOG` | `info` | Tracing `EnvFilter` directive. |

> **Security note (MVP):** the server binds loopback-only. LAN exposure requires auth, which is post-MVP. Cost cap is plugin-only — see `crates/openlet-plugins/test-quota-stub/` for a reference implementation.

## Architecture

```
┌─────────────────┐    SSE + REST    ┌─────────────────────────────────────┐
│  TUI (Ink/React)├─────────────────►│  axum router (openlet-server)       │
└─────────────────┘                  │     │                               │
                                     │     ▼                               │
                                     │  ConversationRuntime<C,T>           │
                                     │     │ (openlet-core)                │
                                     │     ▼                               │
                                     │  six adapter traits ───►  adapters: │
                                     │  ModelProvider              openai  │
                                     │  MemoryStore                sqlite  │
                                     │  ArtifactStore              localfs │
                                     │  ToolExecutor               shell   │
                                     │  EventSink                  bus     │
                                     │  PermissionManager          config  │
                                     └─────────────────────────────────────┘
```

See [`docs/architecture.md`](docs/architecture.md) for crate boundaries, data flow, and the adapter trait contract.

## Workspace layout

```
openlet-ai/
├── Cargo.toml                              # workspace root
├── deny.toml                               # cargo-deny policy
├── rust-toolchain.toml                     # stable channel
└── crates/
    ├── openlet-core/                       # domain types, six adapter traits, runtime
    ├── openlet-adapters/                   # local impls (sqlite, localfs, shell, openai)
    ├── openlet-protocol/                   # utoipa-derived HTTP DTOs
    ├── openlet-plugin-api/                 # stable plugin surface
    ├── openlet-plugin-registry/            # compile-time plugin list
    ├── openlet-plugins/core-agents/        # built-in agent plugins
    ├── openlet-server/                     # axum binary + audit subcommand
    └── openlet-test-mock-provider/         # in-process OpenAI-compat replay
└── tui/                                    # Ink/React TUI (npm package: openlet)
```

## Custom agents

Agents are defined in code via the plugin API. Walkthrough in
[`docs/custom-agents.md`](docs/custom-agents.md). The shipped reference agent
(an indexer stub) lives in `crates/openlet-plugins/core-agents/`.

## CLI

```bash
openlet-server                                  # serve (default)
openlet-server serve --bind 0.0.0.0:8787        # explicit bind
openlet-server audit --session-id <UUID>        # pretty-print JSONL session log
openlet-server audit --session-id <UUID> \
    --format json --from 2026-05-23T10:00:00Z   # filter + pipe-friendly
openlet-server --help
```

The audit command applies a defense-in-depth redaction pass over the on-disk
JSONL — useful for support handoff and CI failure forensics.

## Pre-PR checklist

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
cargo audit
( cd tui && npm run typecheck && npm test && npm pack --dry-run )
```

See [`CONTRIBUTING.md`](CONTRIBUTING.md) for the full developer flow and
[`docs/testing-conventions.md`](docs/testing-conventions.md) for the
integration-test stack (rstest, proptest, wiremock) and conventions.

## Plan

Implementation tracker: `plans/20260523-1414-openlet-agent-core-mvp/plan.md`.
Amendments live in `amendments-after-red-team.md` and `amendments-plugin-system.md`
and override individual phase files on conflict.

## License

Apache-2.0
