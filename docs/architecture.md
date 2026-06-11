# Architecture

A snapshot of the openlet-ai runtime as it lands at the end of Phase 8.

## Crate boundaries

```
openlet-core         domain types, six adapter traits, runtime, projection
openlet-protocol     OpenAPI DTOs (utoipa derive)
openlet-adapters     local impls: openai-compat, sqlite, localfs, localshell, bus, config-perm
openlet-plugin-api   stable plugin trait + PluginContext (CoreApi facade)
openlet-plugin-registry   compile-time plugin list
openlet-plugins/core-agents   built-in agent definitions (general, indexer)
openlet-server       axum router, AppState, AuthN, audit subcommand
openlet-test-mock-provider   in-process OpenAI-compat replay (parity tests)
tui/                 SolidJS (@opentui) TUI client, ships as `openlet` on npm
```

The runtime is split top-down: `openlet-core` knows nothing about HTTP or
filesystems. `openlet-server` wires concrete adapters into the runtime
and exposes the result as REST + SSE.

## Six adapter traits

`openlet-core::adapters` defines the entire surface the runtime depends on:

| Trait | Live impl | Purpose |
|---|---|---|
| `ModelProvider` | `openai_compat::OpenAiCompatProvider` | Streamed `chat_stream` against OpenRouter |
| `MemoryStore` | `sqlite::SqliteMemoryStore` | Sessions, messages, parts (durable) |
| `ArtifactStore` | `localfs::LocalFsArtifactStore` | Per-session blob bucket (sha256-keyed) |
| `ToolExecutor` | `localshell::LocalShellToolExecutor` + tool registry | Bash, file ops, grep, glob |
| `EventSink` | `bus::BroadcastBus` (with sqlite `event_repo`) | Live SSE channel + replay-from-table |
| `PermissionManager` | `config_perm::ConfigPermissionMgr` | Always/ask/never rulesets, deferred resolution |

A new deployment swaps adapters wholesale (e.g. cloud impl for `MemoryStore`)
without touching `openlet-core` or routes.

## Data flow — single turn

```
TUI ── POST /v1/sessions/:id/turns ──► axum route
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
                                          ├─ ToolExecutor.run → tool result Part
                                          └─ MemoryStore.append(assistant message)
```

Every event the SSE channel emits is also persisted to `events` so a
disconnected client can `GET /v1/sessions/:id/events?after=N` to catch up.

## Error flow

A typed error in any adapter (`ProviderError`, `MemoryError`, …) carries a
closed-set `FailureClass`. `openlet-server::AppError` maps each variant to
a stable HTTP status + slug, logs the class via `tracing` (in
`IntoResponse`), and returns an `ErrorDto`. No free-form `Other(String)`
escape hatch — adding a class requires editing the enum.

## Audit and forensics

The `LocalfsSessionLogger` writes a JSONL mirror of every event under
`<data_dir>/sessions/<id>.jsonl`, redacting on the way in (key allowlist
+ regex for bearer / `sk-…` tokens). The `openlet-server audit`
subcommand reads it back, applies the same redactor a second time
(defense-in-depth), and pretty-prints or dumps JSON.

## Testing

- Unit tests live next to the code they cover.
- Adapter integration tests in `crates/openlet-adapters/tests/`.
- Parity tests drive the real `OpenAiCompatProvider` against
  `openlet-test-mock-provider` — no network, no API key, byte-exact
  control over chunking and headers.

## What changes after MVP

Slated post-MVP and out of scope here: cloud adapters (S3 artifacts,
Postgres memory), retriever / RAG, multi-tenant auth, Sixel image
rendering, single-binary TUI distribution.
