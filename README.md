# Openlet AI

Standalone Rust agent runtime exposing REST + SSE, paired with an Ink/React TUI client.

> **Status:** Phase 1 (Foundation). Workspace skeleton, locked adapter trait surface, and a single `/v1/health` endpoint behind utoipa-generated OpenAPI 3.x docs. Business logic lands in Phase 2+.

## Quickstart

```bash
# Build everything
cargo build --workspace

# Run the server (defaults to 127.0.0.1:8787)
cargo run -p openlet-server

# In another terminal:
curl http://127.0.0.1:8787/v1/health
# {"ok":true,"version":"0.1.0"}

# Open Swagger UI:
open http://127.0.0.1:8787/doc
```

## Configuration

All config is environment-driven (TOML support lands in Phase 8):

| Env var | Default | Purpose |
|---|---|---|
| `OPENLET_BIND` | `127.0.0.1:8787` | TCP bind address. Loopback-only by default. |
| `OPENLET_DATA_DIR` | `~/.openlet` | Persistent data root (sqlite, artifacts). |
| `OPENROUTER_API_KEY` | _(unset)_ | OpenRouter / OpenAI-compat credentials. |
| `OPENLET_DEFAULT_MODEL` | `anthropic/claude-sonnet-4-6` | Default chat model. |
| `OPENLET_MAX_COST_USD` | `5.00` | Per-session hard limit (USD). |
| `OPENLET_LOG_FORMAT` | `json` | `json` or `pretty`. |
| `RUST_LOG` | `info` | Tracing `EnvFilter` directive. |

> **Security note (MVP):** the server binds loopback-only. LAN exposure requires auth, which is post-MVP.

## Workspace layout

```
openlet-ai/
├── Cargo.toml                          # workspace root
├── rust-toolchain.toml                 # stable channel
└── crates/
    ├── openlet-core/                   # domain types, six adapter traits, Config, errors
    ├── openlet-adapters/               # local impls (Phase 1: stubs)
    ├── openlet-protocol/               # utoipa-derived HTTP DTOs
    ├── openlet-plugin-api/             # stable plugin surface
    ├── openlet-plugin-registry/        # compile-time plugin list
    └── openlet-server/                 # axum binary + AppState
```

## Adapter traits (locked in Phase 1)

| Trait | Phase 1 status | Real impl phase |
|---|---|---|
| `ModelProvider` | stub returns `Unimplemented` | Phase 3 |
| `MemoryStore` | stub returns `Unimplemented` | Phase 2 |
| `ArtifactStore` | stub returns `Unimplemented` | Phase 2 |
| `ToolExecutor` | stub returns `Unimplemented` | Phase 4 |
| `EventSink` | broadcast subscribe wired; publish stubbed | Phase 5 |
| `PermissionManager` | stub returns `Unimplemented` | Phase 4 |

## CLI

```bash
openlet-server                # Serve (default subcommand)
openlet-server serve          # Same as above
openlet-server audit          # Reserved for Phase 8
openlet-server --help
```

## Plan

See `plans/20260523-1414-openlet-agent-core-mvp/plan.md`. Phase amendments live in `amendments-after-red-team.md` and `amendments-plugin-system.md` and override individual phase files on conflict.

## License

Apache-2.0
