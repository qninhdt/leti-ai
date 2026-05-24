# Deployment

MVP target: a single laptop, single user, loopback only. This document
covers the env-var surface, file layout, and the safety constraints baked
into the bind defaults. Cloud / multi-tenant deployment is post-MVP and
explicitly out of scope.

## Process model

`openlet-server` is a single binary. It owns its sqlite file, its artifact
directory, and its session-log directory. There are no required external
services in MVP — OpenRouter is the one network dependency.

The TUI (`openlet`, npm) is a separate process that talks to the server
over loopback HTTP + SSE. The TUI is optional — anything that speaks the
OpenAPI shape (see `/v1/doc/openapi.json`) works.

## Environment variables

| Var | Default | Required? | Purpose |
|---|---|---|---|
| `OPENLET_BIND` | `127.0.0.1:8787` | no | TCP bind. Loopback only by default. |
| `OPENLET_DATA_DIR` | `~/.openlet` | no | Sqlite + artifacts + session logs root. |
| `OPENROUTER_API_KEY` | _(unset)_ | yes (for live model) | Bearer token for OpenRouter. |
| `OPENLET_DEFAULT_MODEL` | `anthropic/claude-sonnet-4-6` | no | Default chat model id. |
| `OPENLET_MAX_COST_USD` | `5.00` | no | Per-session hard limit. |
| `OPENLET_LOG_FORMAT` | `json` | no | `json` or `pretty`. |
| `OPENLET_WORKSPACE` | `<data_dir>/workspace` | no | Default agent workspace root. |
| `RUST_LOG` | `info` | no | Tracing `EnvFilter` directive. |

## Filesystem layout

```
$OPENLET_DATA_DIR/
├── db.sqlite                 # MemoryStore + event repo
├── artifacts/
│   └── <session-id>/<sha>    # ArtifactStore content-addressed bucket
├── sessions/
│   └── <session-id>.jsonl    # JSONL audit log (redacted on write)
└── workspace/                # default agent workspace (override per-agent)
```

Sessions live in sqlite as the source of truth; the JSONL mirror exists
for support / forensics and is the input to `openlet-server audit`.

## Bind safety

The default `127.0.0.1:8787` is enforced by the loaded `Config`, not by
any iptables rule. To expose to LAN, set `OPENLET_BIND` explicitly. There
is no auth in MVP — exposing the port to anything beyond loopback gives
arbitrary tool execution to any reacher. Don't do it in MVP.

## OpenRouter / OpenAI-compat configuration

The shipped provider points at `https://openrouter.ai/api/v1`. To use a
self-hosted gateway (LiteLLM, vLLM with the OpenAI-compat shim, etc.),
construct `OpenAiCompatProvider::new(base_url, api_key)` directly — for
now this requires editing `openlet-server::main` (env-driven base-URL
override is post-MVP).

For local dry runs without a real model:

```bash
cargo run -p openlet-test-mock-provider --bin mock-openai-service
```

Watch the printed `base_url` and edit the provider construction to point
at it. The mock returns canned `simple_text` by default; embed
`PARITY_SCENARIO:<name>` in the user message text to pick another
scenario (see `crates/openlet-test-mock-provider/src/scenarios.rs`).

## Resource limits

- `OPENLET_MAX_COST_USD` caps the per-session spend.
- The `LocalShellExecutor` runs commands with the workspace as `cwd` and
  inherits a scrubbed env (allowlist applied).
- File tools are workspace-scoped — `..` and absolute-path probes are
  rejected at the `LocalFilesystem` boundary.

## Logging

`tracing-subscriber` defaults to JSON-shaped output suitable for piping
into a log shipper. Switch to `OPENLET_LOG_FORMAT=pretty` for local
development. Errors are emitted with structured fields including
`class` (the `FailureClass` slug) so dashboards can group failures
without parsing free-form text.

## Pre-PR pipeline

```
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
cargo audit
( cd tui && npm run typecheck && npm test && npm pack --dry-run )
```

Any of these failing blocks merge.
