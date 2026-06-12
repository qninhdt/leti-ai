# System Architecture

_Last updated: 2026-06-11. Supersedes the Phase-8 snapshot in `architecture.md`._

## Crate boundaries

```
openlet-core            domain types, port traits, runtime, projection, dispatch
openlet-protocol        wire DTOs (utoipa derive)
openlet-adapters        local impls: openai + openrouter, sqlite, localfs,
                        localshell, broadcast bus, config-perm
openlet-plugin-api      stable Plugin trait + PluginContext (CoreApi facade) + hook IO
openlet-plugin-registry install_all — drains registrations into hook chains
openlet-plugins/*       core-tools (8 builtins), core-agents, test-quota-stub
openlet-server          axum router, AppState, auth + workspace middleware,
                        metrics, evidence scrubber, subagent spawner/driver, binary
openlet-test-mock-provider  in-process OpenAI-compat mock (wire capture, keyless)
tui/                    TypeScript terminal client (Ink→Solid migration in progress)
```

`openlet-core` knows nothing about HTTP or filesystems. `openlet-server` wires
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
| `WorkspaceResolver` | `StaticWorkspaceResolver` | caller+id → workspace AppState, ownership 403 | takes the principal |
| `Authenticator` | `LocalDevAuthenticator` | inbound identity (zero-trust) | cloud JWKS impl plugs in |
| `CredentialProvider` | `NoopCredentialProvider` | outbound SA credential | cloud SA issuer plugs in |

Widening is additive default methods where possible; cloud impls live in the
openlet repo and satisfy the contract spec in `docs/integration-guide.md`.

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

## Identity & multi-tenancy (Phases 6–7)

Mount order (outermost→innermost): BodyLimit → CORS → Trace → `AuthLayer` →
`WorkspaceRoutingLayer` → handler. `AuthLayer` runs first and injects the
canonical `AuthPrincipal`; the workspace layer resolves `(principal, ws_id)` →
`AppState`, returning 403 on an ownership mismatch. Local profile uses the dev
authenticator + single-tenant static resolver; the cloud binary supplies its
own via `RouterBuilder::build_with_auth` and a dynamic resolver. Runtime profile
(`OPENLET_RUNTIME_PROFILE`) fails closed for `cloud` without a real authenticator.

## Plugins & hooks

`install_all` drains every plugin's registrations into sorted hook chains + a
tool registry + agent definitions + an optional provider. 14 hook kinds
(before/after turn, chat params/messages/headers, tool call, permission ask,
cost tick, step finish, compaction, …). Every dispatch site is panic/timeout
isolated: a fault synthesizes `Denied{fault}` and publishes a durable
`PluginError`. Quota/cost-cap is the cost-tick seam (`test-quota-stub`), not a
trait. The eight built-in tools ship as `core-tools`.

## Subagents

`subagent_task` tool → `RuntimeSubagentSpawner` (server) admits via
`plan_subagent_spawn` (depth + per-root quota), persists the child session,
seeds the objective, and drives a nested `run_loop` in `subagent_driver`. Child
cost rolls up to the parent's ledger; the parent's cancel token is the child's
parent (`child_token`), so cancellation cascades. Terminal snapshots are cached
so a lost finalize race can't strand `await_completion`.

## Error flow

Typed errors per layer carry a closed-set `FailureClass`. `AppError` maps each
variant to a stable HTTP status + slug, logs the class via `tracing`, returns an
`ErrorDto`. No `Other(String)` escape hatch.

## Observability (Phase 10)

Correlated spans: `request` (request_id) → `turn` (session_id, turn_id) →
dispatch/provider/compaction, flowing into JSON logs. Metrics via the `metrics`
facade — emission is a no-op until a Prometheus recorder is installed, and
`/metrics` binds only when `OPENLET_METRICS_BIND` is set (separate listener, no
per-workspace label on the open scrape).

## Persistence

SQLite via sqlx, migrations `0001`–`0008`. `SessionMeta` = explicit columns +
JSON blobs (`extensions`, `capabilities`). Durable event ids assigned/persisted/
broadcast under one lock (ordering guarantee), seeded from `MAX(id)` across
restart. Artifacts on localfs, sha256-keyed, metadata in sqlite. The
`LocalfsSessionLogger` writes a redacted JSONL mirror; the `audit` subcommand
replays it (re-redacting, defense-in-depth).

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
  openlet repo, built against these seams.
