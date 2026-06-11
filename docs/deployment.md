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
| `OPENLET_MODEL_BASE_URL` | `https://openrouter.ai/api/v1` | no | Model API base URL — honored on the serve path. Point at a self-hosted gateway or the in-process mock. |
| `OPENLET_MAX_COST_USD` | `5.00` | no | Per-session hard limit. |
| `OPENLET_LOG_FORMAT` | `json` | no | `json` or `pretty`. |
| `OPENLET_METRICS_BIND` | _(unset)_ | no | If set (e.g. `127.0.0.1:9464`), serves Prometheus metrics at `GET /metrics` on that address. Unset → metrics fully off. |
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

The shipped provider defaults to `https://openrouter.ai/api/v1`. To use a
self-hosted gateway (LiteLLM, vLLM with the OpenAI-compat shim, etc.), set
`OPENLET_MODEL_BASE_URL` to its base URL — the serve path honors it, no
source edit needed. Unset falls back to the OpenRouter default. The
credential still flows only as the `Authorization` header.

For local dry runs without a real model, the launcher wires the mock for
you:

```bash
./openlet-ai --mock
```

To do it by hand, start the mock and point the server at its printed
`base_url` verbatim (it already ends in `/v1` — do NOT append another):

```bash
cargo run -p openlet-test-mock-provider --bin mock-openai-service
# then: OPENLET_MODEL_BASE_URL=<printed base_url> cargo run -p openlet-server
```

The mock returns canned `simple_text` by default; embed
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

Every HTTP request runs inside a `request` span (fresh `request_id`) and
every agent turn inside a `turn` span (`session_id` + `turn_id`), with
nested spans on tool dispatch, the provider call, and compaction — so a
single turn is traceable end-to-end in the JSON logs.

## Metrics

openlet-ai is software, not an infra bundle: **running it locally needs no
Prometheus and no docker-compose.** Metric emission compiles in but is a
no-op until a recorder is installed, and the recorder + `/metrics`
endpoint only start when `OPENLET_METRICS_BIND` is set.

```bash
# No metrics — plain local run.
cargo run -p openlet-server -- serve

# Opt in: serve Prometheus metrics on a separate loopback port.
OPENLET_METRICS_BIND=127.0.0.1:9464 cargo run -p openlet-server -- serve
# then point a Prometheus scraper at http://127.0.0.1:9464/metrics
```

Exposed metrics: `openlet_turns_total`, `openlet_tokens_total{kind}`,
`openlet_cost_usd_total`, `openlet_provider_retries_total`,
`openlet_tool_executions_total{tool,outcome}`, `openlet_compactions_total`,
`openlet_plugin_faults_total{hook}`.

The endpoint binds on its own listener — it is NEVER mounted on the public
app router (no auth/body-limit/CORS coupling). **Security:** the open
scrape exposes AGGREGATE values only — no per-`workspace` label, because
that label set would enumerate every tenant (cross-tenant spend leak +
Prometheus cardinality DoS). Cloud deployments scrape `/metrics` on an
internal port; per-workspace breakdown belongs behind an authenticated
admin scrape, not this endpoint.

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
