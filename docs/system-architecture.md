# System Architecture

_Last updated: 2026-07-17. See `architecture.md` for the integration boundary._

## Crate boundaries

```
leti-core            domain types, port traits, runtime, projection, dispatch
leti-protocol        wire DTOs (utoipa derive)
leti-adapters        local impls: openai + openrouter, sqlite, localfs,
                        localshell, broadcast bus, config-perm
leti-plugin-api      stable Plugin trait + PluginContext (CoreApi facade) + hook IO
leti-plugin-registry install_all — drains registrations into hook chains
leti-plugins/*       core-tools (8 builtins), core-agents, test-quota-stub
leti-server          axum router, AppState, auth + workspace middleware,
                        metrics, evidence scrubber, subagent spawner/driver, binary
leti-test-mock-provider  in-process OpenAI-compat mock (wire capture, keyless)
tui/                    TypeScript terminal client (Ink→Solid migration in progress)
```

`leti-core` knows nothing about HTTP or filesystems. `leti-server` wires
concrete adapters into the runtime and exposes REST + SSE.

## Port traits (the adapter surface)

| Trait | Local impl | Purpose | Cloud-readiness |
|---|---|---|---|
| `ModelProvider` | `OpenAiProvider` / `OpenRouterProvider` | streamed `chat_stream`, pricing, capabilities, list_models | impl-agnostic post-OpenRouter |
| `MemoryStore` | `SqliteMemoryStore` | sessions/messages/parts; **paginated** list methods | `list_*_paged` (Page/PageResult) |
| `ArtifactStore` | `LocalFsArtifactStore` | per-session blobs; **streaming** + optional **presign** | `get_stream`/`put_stream`/`presign` |
| `EventSink` | `BroadcastBus` (+ sqlite repo) | SSE channel + replay; **routing key** + **delivery semantics** | `publish_routed`, `delivery_semantics()` |
| `PermissionManager` | `ConfigPermissionMgr` | always/ask/never rulesets, deferred resolution | async, opaque AskId |
| `Filesystem` | `LocalFilesystem` | workspace file IO (read/write/glob/grep), jailed to workspace root | swappable for remote workspace |

Widening is additive default methods where possible; cloud impls live in the
leti repo and satisfy the contract spec in `docs/integration-guide.md`.

## Data flow — a single turn

```
client ── POST /v1/session/:id/prompt_async ──► route (auth → workspace layers)
                                                  │ append user msg, claim slot, 202
                                                  ▼ tokio::spawn
                                        ConversationRuntime::run_loop  [turn span]
                                          ├─ project conversation (caps-aware)
                                          ├─ chat_stream_with_retry  [provider span]
                                          │     deltas → Processor → Parts
                                          │     part.delta (transient) → SSE
                                          ├─ on tool_use: dispatcher  [dispatch span]
                                          │     permission.check → ToolRegistry.dispatch
                                          │     before/after hook chains
                                          ├─ append tool results, continue
                                          └─ under pressure: run_compaction [span]
```

Durable events persist to the `events` table; a disconnected client resumes via
`GET /v1/event` with `Last-Event-ID`, and hydrates history with
`GET /v1/session/:id/messages` (the Part union).

## Host authentication and multi-tenancy

Authentication, tenant lookup, and ownership authorization belong to the host.
The reference server includes loopback-oriented auth and static workspace
routing so it can run standalone; a cloud host supplies its own verifier and
resolver around `RouterBuilder`. The host must authenticate before workspace
lookup and must authorize session creation and interaction-mode changes.
Identity needed by a host adapter travels through opaque `TurnExtensions`; it
is not an engine policy type.

## Plugins & hooks

`install_all` drains every plugin's registrations into sorted hook chains + a
tool registry + agent definitions + an optional provider. 14 hook kinds
(before/after turn, chat params/messages/headers, tool call, permission ask,
cost tick, step finish, compaction, …). Every dispatch site is panic/timeout
isolated: a fault synthesizes `Denied{fault}` and publishes a durable
`PluginError`. Quota/cost-cap is the cost-tick seam (`test-quota-stub`), not a
trait. The built-in core tool set ships as `core-tools`; `web_fetch` is
registered only when the host injects its outbound-network fetcher and is
permission-gated to Ask by default.

## Subagents

`subagent_task` tool → `RuntimeSubagentSpawner` (server) admits via
`plan_subagent_spawn` (depth + per-root quota), persists the child session,
seeds the objective, and drives a nested `run_loop` in `subagent_driver`. Child
cost rolls up to the parent's ledger; the parent's cancel token is the child's
parent (`child_token`), so cancellation cascades. Terminal snapshots are cached
so a lost finalize race can't strand `await_completion`.

The SQLite execution ledger is the restart-safe lifecycle authority. A boot
converts live entries to `interrupted(process_restart)` and requires explicit
continue rather than repeating potentially side-effecting provider/tool work.
Sibling inbox records persist before notification and are acknowledged only
after being written as untrusted child-transcript input.

## Error flow

Typed errors per layer carry a closed-set `FailureClass`. `AppError` maps each
variant to a stable HTTP status + slug, logs the class via `tracing`, returns an
`ErrorDto`. No `Other(String)` escape hatch.

## Observability (Phase 10)

Correlated spans: `request` (request_id) → `turn` (session_id, turn_id) →
dispatch/provider/compaction, flowing into JSON logs. Metrics via the `metrics`
facade — emission is a no-op until a Prometheus recorder is installed, and
`/metrics` binds only when `LETI_METRICS_BIND` is set (separate listener, no
per-workspace label on the open scrape).

## Persistence

SQLite via sqlx, migrations `0001`–`0017`. `SessionMeta` = explicit columns +
JSON blobs (`extensions`, `capabilities`). Durable event ids assigned/persisted/
broadcast under one lock (ordering guarantee), seeded from `MAX(id)` across
restart. Artifacts on localfs, sha256-keyed, metadata in sqlite. The
`LocalfsSessionLogger` writes a redacted JSONL mirror; the `audit` subcommand
replays it (re-redacting, defense-in-depth).

Session interaction mode is explicit and defaults to `Interactive`. Detached
permission checks emit durable authorization events, while top-level detached
turns are not automatically re-driven after process restart.

## Infrastructure & CI (Phases 13–14)

Multi-stage Dockerfile (cargo-chef cache → release build → non-root debian-slim
with `/v1/health` HEALTHCHECK + `/data` volume). Compose: tracked base (no host
ports), gitignored local override (port remap + mock profile), env separation
via `--env-file`. CI: `ci.yml` (fmt/clippy/test/deny/audit + TUI + contract
drift) on every PR; `nightly.yml` (gated real-LLM acceptance + scrubbed
evidence); `image.yml` (build + healthcheck smoke).

## Known in-progress / out of scope

- TUI is mid Ink→Solid migration; its agent-surface fixes (Phase 8/9) are
  deferred. The server-side `GET …/messages` it needs is done.
- Cloud adapter implementations (Postgres/S3/Kafka, JWKS, SA issuer) live in the
  leti repo, built against these seams.
