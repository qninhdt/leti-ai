# Project Overview (PDR)

_Last updated: 2026-06-11_

## What openlet-ai is

A standalone Rust **AI agent runtime** exposing a REST + SSE API, paired with
a terminal (TUI) client. It drives a multi-step LLM tool-use loop: a model
streams a turn, the runtime dispatches the tools the model requests, appends
their results, and continues until the model ends the turn — persisting every
message/part to SQLite and broadcasting live events over SSE.

It is built to run two ways from one codebase:

- **Local**: `./openlet-ai` (or `docker compose up`) — loopback-only, no auth
  server, SQLite + local filesystem, OpenRouter or an in-process mock model.
- **Cloud-integration-ready**: the openlet team plugs cloud adapters
  (Postgres/S3/Kafka, JWKS auth, service-account credentials) into the
  trait/plugin seams **without forking** openlet-ai.

## Goals

- A correct, observable agent loop: streaming, tool calls, permissions,
  compaction, subagents, cost tracking — all persisted and resumable.
- A hexagonal **port/adapter** architecture so every backend (model, storage,
  events, permissions, filesystem) is swappable behind a trait.
- A **plugin** surface (hooks + tools + agents) so integrators extend behavior
  without editing core.
- Cloud-readiness as **seams + local defaults + a contract spec**, not bundled
  cloud implementations.

## Non-goals

- Shipping cloud adapter implementations (those live in the openlet repo).
- Requiring infrastructure (Prometheus, a database server, an auth server) to
  run locally — local is plain software.
- A graphical UI; the reference client is a terminal app.

## Primary users

- **End developers** running the local agent against OpenRouter.
- **The openlet cloud team**, who implement cloud adapters against the seams.
- **Plugin authors** extending tools/agents/hooks.

## Capability summary

| Capability | State |
|---|---|
| Streaming multi-step tool-use loop | Implemented |
| SQLite persistence + SSE resume (Last-Event-ID) | Implemented |
| Permission gate (allow/deny/ask + always-rules) | Implemented |
| Context compaction under pressure | Implemented |
| Subagents (spawn/run/await/cost-rollup/cancel-cascade) | Implemented |
| Provider retry/backoff + per-session model | Implemented |
| OpenRouter adapter (attribution, routing, models fallback, cost) | Implemented |
| Inbound auth seam + outbound credential seam | Implemented (local defaults; cloud impls external) |
| Widened storage/event adapter contracts (pagination, streaming, routing) | Implemented (local impls; cloud external) |
| Telemetry: correlated spans + Prometheus metrics | Implemented (dormant until opted in) |
| Docker/Compose + env separation | Implemented |
| CI/CD (PR checks + gated nightly + image) | Implemented |
| TUI agent surfaces (tool render, resume, ask-user) | **In progress** (Ink→Solid migration) |

## Architecture in one paragraph

`openlet-core` holds the IO-free domain + the port traits + the conversation
runtime. `openlet-adapters` holds the local implementations (sqlite, localfs,
localshell, broadcast bus, OpenAI/OpenRouter providers). `openlet-server`
composes them into an axum HTTP/SSE surface with auth + workspace-routing
middleware. `openlet-protocol` is the wire DTO set. Plugins
(`openlet-plugin-*`) register tools/agents/hooks through a stable API; the
built-in core tools ship as the `core-tools` plugin, with outbound `web_fetch`
opt-in at host wiring. The TUI (`tui/`) is
a separate TypeScript client.

See `docs/system-architecture.md` for the full picture.

## Open questions (owner / openlet team)

Carried from the rehabilitation work; tracked in the final report:

- SA credential scope & issuance (per-workspace token vs one SA + claim).
- Cost-ledger ownership (self-contained vs a future openlet quota service).
- Whether the caller set includes leti→agent calls (affects `PrincipalType`).
- Presigned-URL capability timing for agent file tools.
