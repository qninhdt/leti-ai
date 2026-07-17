# Architecture

The leti-ai engine is a reusable agent runtime. The reference binary is a
single-user loopback deployment; a host can embed the same library, replace
ports at its composition root, merge its own routes and middleware, and add
plugins.

## Crate boundaries

```
leti-core         domain types, six adapter traits, runtime, projection
leti-protocol     OpenAPI DTOs (utoipa derive)
leti-adapters     local impls: openai-compat, sqlite, localfs, localshell, bus, config-perm
leti-plugin-api   stable plugin trait + PluginContext (CoreApi facade)
leti-plugin-registry   compile-time plugin list
leti-plugins/core-agents   built-in agent definitions (general, indexer)
leti-server       axum router, AppState, AuthN, audit subcommand
leti-test-mock-provider   in-process OpenAI-compat replay (parity tests)
tui/                 SolidJS (@opentui) TUI client, ships as `leti` on npm
```

The runtime is split top-down: `leti-core` knows nothing about HTTP,
authentication, tenants, principals, or cloud services. It defines ports and
the loop. `leti-server` supplies the reference HTTP/SSE composition and local
adapters. A host owns authentication and authorization middleware and wires
cloud implementations at its own composition root.

The core-purity rule is enforced by an integration test: `leti-core` may not
depend on filesystem, network, database, cloud, or identity implementations.
Host-owned per-turn data uses `TurnExtensions`; the engine transports it from
the loop to permission checks, tools, and subagents but never interprets,
persists, serializes, or logs it.

## Composition root versus plugins

Use the composition root for infrastructure and policy-bearing ports:
`ModelProvider`, `MemoryStore`, `ArtifactStore`, `EventSink`, `Filesystem`,
and `PermissionManager`. Use plugins for agent definitions, tools, hooks, and
provider-independent behavior. A cloud host should merge its auth, ownership,
and business routes around `RouterBuilder`; it should not add those concerns
to `leti-core`.

## Six adapter traits

`leti-core::adapters` defines the entire surface the runtime depends on:

| Trait | Live impl | Purpose |
|---|---|---|
| `ModelProvider` | `openai_compat::OpenAiCompatProvider` | Streamed `chat_stream` against OpenRouter |
| `MemoryStore` | `sqlite::SqliteMemoryStore` | Sessions, messages, parts (durable) |
| `ArtifactStore` | `localfs::LocalFsArtifactStore` | Per-session blob bucket (sha256-keyed) |
| `Filesystem` | `LocalFilesystem` | Workspace file IO — read/write/glob/grep, jailed to the workspace root |
| `EventSink` | `bus::BroadcastBus` (with sqlite `event_repo`) | Live SSE channel + replay-from-table |
| `PermissionManager` | `config_perm::ConfigPermissionMgr` | Always/ask/never rulesets, deferred resolution |

Tool execution is not an adapter trait. Tools implement the `Tool` /
`ErasedTool` contract and register into a `ToolRegistry`; built-in tools
(bash, file ops, grep, glob) reach the workspace through the per-agent
`Filesystem` handle (`AgentResources.fs`).

`web_fetch` is deliberately also outside the six-trait adapter surface. Its
tool-local `WebFetcher` seam is optionally injected by the host; the reference
server wires `ReqwestWebFetcher`, while network-free integrators can omit it
and do not register the tool. The production implementation permits only
public `http`/`https` destinations, pins each DNS-resolved IP before connecting
and rechecks redirects, and size-caps output. Its `web_fetch:**` permission
rule defaults to Ask so model-controlled URLs cannot silently exfiltrate data.

A new deployment swaps adapters wholesale (e.g. cloud impl for `MemoryStore`)
without touching `leti-core` or routes.

## Data flow — single turn

```
TUI ── POST /v1/session/:id/prompt_async ──► axum route
                                          │
                                          ▼
                                ConversationRuntime::run_turn
                                          │
                                          ├─ MemoryStore.append(user message)
                                          ├─ project_for_llm()
                                          ├─ ModelProvider.chat_stream() ─►  SSE bytes
                                          │       │
                                          │       ▼
                                          │   Processor: SseFrame → ChatDelta → Part
                                          ├─ EventSink.publish(part_created, …)  ─► SSE channel
                                          ├─ Tool dispatch → PermissionManager.check
                                          ├─ ToolRegistry.dispatch → tool result Part
                                          └─ MemoryStore.append(assistant message)
```

Every event the SSE channel emits is also persisted to `events` so a
disconnected client can `GET /v1/event?session=<uuid>` with a `Last-Event-ID`
header to catch up. Replay pages to a captured high-water mark; a client that
receives the synthetic `lagged` frame reconnects with the same durable cursor.

## Interaction modes and subagents

Sessions are `Interactive` by default. A session may explicitly opt into
`Detached { on_ask: Allow|Deny }` through the session API or the reference
server's `--detached --on-ask allow|deny` defaults. Detached mode resolves
ordinary Ask outcomes without waiting for a client, while explicit Deny rules
still win. `web_fetch` and destructive shell subjects are not blanket-upgraded
by `on_ask=allow`; they require an explicit allow rule. Untrusted injected
turns remain fail-closed. Every detached permission check emits a durable
authorization event, including direct Danger-mode allows.

Top-level detached turn replay after a process restart is intentionally not
implemented yet. Existing boot recovery handles durable background-delivery
leases and marks other live execution records interrupted; it does not
re-drive an interrupted top-level turn.

Before every provider request, the shared request-preparation boundary reloads
the durable transcript, collects typed runtime reminders, persists any new
ones atomically, then projects the effective history. Runtime reminders and
compaction boundaries are typed `Part`s: they may affect model context but are
never treated as human-authored transcript text.

`subagent_task` is foreground by default. Foreground calls join their child
and return its final body only through the originating tool result; contiguous
parallel-safe calls run as ordered concurrent waves around unsafe-tool
barriers. Background calls return a task/child-session descriptor immediately.
The running task card can also call the parent-scoped background endpoint:
a shared compare-and-swap changes only delivery ownership
(`ForegroundWaiting → Background`), never the child task or session. A racing
settlement selects the matching terminal owner, so the original tool result
and the outbox cannot both expose the child output.
On terminal settlement the server writes one typed parent reminder and its
delivery outbox row in one SQLite transaction, emits metadata-only SSE
lifecycle frames, and schedules a fail-closed autonomous parent turn without
adding a user bubble. The outbox is durable and uses `pending`, `leased`, and
`delivered` states: an atomic claim assigns a unique lease token, and each
live delivery turn renews only its own lease heartbeat. The row is acknowledged
as `delivered` only after that parent turn succeeds; a turn error releases its
matching token back to `pending`, while a crash leaves the lease to expire.
Startup and the periodic reconciler claim both pending and expired-lease rows,
so a stale worker cannot acknowledge, release, or renew a later attempt.

Subagent executions are independently durable. Each child transcript is
reusable while every invocation receives a distinct execution id and lifecycle
row (`pending → running → terminal`). Boot marks live rows `interrupted` with
`process_restart`; it never blindly replays provider work. A user or tool can
explicitly continue the child, preserving its transcript but creating a new
execution. `subagent_list`, `task_status`, cancel, interrupt and continue all
read that durable source when the in-process registry is unavailable. Sibling
messages are also persisted before wake-up and acknowledged only after their
untrusted payload is appended to the receiving child transcript.

Compaction persists a typed request boundary. The request wording is injected
only for the compaction provider call; the generated assistant text remains a
normal visible summary, while the projection uses its paired compaction part
to replace older model history exactly once. Attempts transition from pending
to committed or failed; failed markers and their partial assistant summary are
suppressed from both normal projection and the human timeline.

## Error flow

A typed error in any adapter (`ProviderError`, `MemoryError`, …) carries a
closed-set `FailureClass`. `leti-server::AppError` maps each variant to
a stable HTTP status + slug, logs the class via `tracing` (in
`IntoResponse`), and returns an `ErrorDto`. No free-form `Other(String)`
escape hatch — adding a class requires editing the enum.

## Audit and forensics

The `LocalfsSessionLogger` writes a JSONL mirror of every event under
`<data_dir>/sessions/<id>.jsonl`, redacting on the way in (key allowlist
+ regex for bearer / `sk-…` tokens). The `leti-server audit`
subcommand reads it back, applies the same redactor a second time
(defense-in-depth), and pretty-prints or dumps JSON.

## Testing

- Unit tests live next to the code they cover.
- Adapter integration tests in `crates/leti-adapters/tests/`.
- Parity tests drive the real `OpenAiCompatProvider` against
  `leti-test-mock-provider` — no network, no API key, byte-exact
  control over chunking and headers.
- Background settlement coverage uses the real SQLite outbox: prove atomic
  claims, token-guarded acknowledgement/release/renewal, retry after a failed
  parent turn, and reconciliation after an expired lease.

## What changes after MVP

Slated post-MVP and out of scope here: cloud adapters (S3 artifacts,
Postgres memory), retriever / RAG, multi-tenant auth, Sixel image
rendering, single-binary TUI distribution.
