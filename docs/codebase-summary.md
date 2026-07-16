# Codebase Summary

_Last updated: 2026-06-11_

A map of the workspace: what each crate owns and where to look.

## Workspace crates

| Crate | Role |
|---|---|
| `openlet-core` | IO-free domain types, port traits, the conversation runtime (turn loop, compaction, subagents, cost, dispatch), tool definitions. Depends on no backend. |
| `openlet-adapters` | Local port implementations: sqlite memory store, localfs artifact store + filesystem, localshell executor, broadcast-bus event sink, OpenAI + OpenRouter providers, and the IP-pinned `ReqwestWebFetcher`. |
| `openlet-protocol` | Wire DTOs (`dto/*`) for the HTTP API + SSE events. The TUI's contract source. |
| `openlet-server` | axum composition: routes, auth + workspace-routing middleware, AppState/builder, metrics, evidence scrubber, the runtime subagent spawner + driver, the binary. |
| `openlet-plugin-api` | The plugin trait + `PluginContext` + hook IO types + `CoreApi` back-channel. |
| `openlet-plugin-registry` | `install_all` — drains plugin registrations into sorted hook chains + tools + agents + an optional provider. |
| `openlet-plugins/core-tools` | Built-in tools as a plugin. `web_fetch` is registered only when the host injects a fetcher, preserving network-free embeddings. |
| `openlet-plugins/core-agents` | Built-in agent definitions (general, indexer, plan). |
| `openlet-plugins/test-quota-stub` | Reference quota plugin: the cost-tick cancel pattern the cloud team forks. |
| `openlet-test-mock-provider` | In-process HTTP mock OpenAI service for keyless tests (captures wire requests). |
| `tui/` | TypeScript terminal client (mid-migration Ink→Solid). |

## openlet-core layout

- `types/` — `session`, `message`, `part`, `event`, `agent`, `permission`,
  `pagination` (Page/PageResult). Plain data, serde + utoipa.
- `adapters/` — the port traits: `model_provider`, `memory_store`,
  `artifact_store` (streaming + presign), `event_sink` (routing + delivery),
  `permission_manager`, `filesystem`, `tool_executor`; plus hooked wrappers.
- `runtime/` — `conversation` (provider call + retry/backoff), `turn_loop` +
  `turn_loop_compaction`, `turn_stream`, `processor`, `compaction`,
  `token_estimate`, `cost`, `question_registry`, `subagent/` (task registry,
  scoped permissions, spawn planning).
- `tools/` — registry, dispatcher, erased tool wrapper, `builtins/`.
- `dispatch.rs` — hook-chain dispatch + fault synthesis (`publish_fault_if_any`).

## openlet-server layout

- `routes/` — `session`, `message` (prompt_async + `GET …/messages`),
  `event` (SSE), `permission`, `question`, `agent`, `model`, `plugin`,
  `diagnostics`, `attachments`, `files`.
- `auth/` — `principal`, `authenticator` (+ `LocalDevAuthenticator`,
  `RuntimeProfile`), `credential` (+ `NoopCredentialProvider`), `layer`.
- `middleware/workspace_routing.rs`, `workspace_resolver.rs` — tenant routing.
- `metrics.rs` — Prometheus recorder + `/metrics` (dormant unless bound).
- `evidence_scrubber.rs` — redaction for real-LLM transcripts.
- `subagent_spawner.rs` + `subagent_driver.rs` — the real subagent path.
- `app_state.rs` / `app_state_builder.rs` / `router.rs` / `main.rs`.

## Request → turn flow

1. `POST /v1/session/:id/prompt_async` appends the user message, claims the
   turn slot, spawns the loop, returns `202`.
2. `run_loop` (core) projects the conversation, calls the provider with
   retry/backoff, streams deltas through the processor into persisted parts +
   transient SSE `part.delta` events.
3. On a tool-use finish, the dispatcher runs the requested tools (permission
   gate + hook chains), appends results, continues.
4. Under context pressure, `run_compaction` summarizes + re-projects.
5. SSE (`GET /v1/event`) streams it live; `GET /v1/session/:id/messages`
   hydrates history on resume.

## Persistence

SQLite via sqlx, migrations `0001`–`0008` (latest: per-session `model`).
`SessionMeta` uses explicit columns + JSON blob fields (`extensions`,
`capabilities`). Artifacts on localfs keyed by sha256(key), metadata in sqlite.

## Where to start for a task

- Change the agent loop → `openlet-core/src/runtime/turn_loop.rs`.
- Add/modify an HTTP route → `openlet-server/src/routes/` + `router.rs`.
- Swap a backend → implement the port trait in `openlet-adapters`.
- Extend behavior → write a plugin against `openlet-plugin-api`.
- Cloud integration → `docs/integration-guide.md`.
