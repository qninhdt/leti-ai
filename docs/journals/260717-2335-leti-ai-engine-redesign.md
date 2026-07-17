---
date: 2026-07-17
topic: leti-ai-engine-redesign
plan: plans/20260716-1616-leti-ai-agent-engine-redesign
status: completed
---

# leti-ai Agent Engine Redesign

## Context

Completed the five-phase redesign that turns the renamed `leti-ai` project
into a business-agnostic agent engine suitable for a standalone loopback
server or an Openlet-hosted integration.

## What changed

- Renamed the workspace, crates, launcher, configuration, TUI, and docs from
  `openlet-*` / `OPENLET_*` to `leti-*` / `LETI_*`; legacy variables now fail
  loudly instead of silently selecting defaults.
- Deleted the in-tree cloud filesystem, its gRPC/proto build path, cloud
  runtime profile, and core cloud credential configuration.
- Added `TurnExtensions`: host-defined, typed, runtime-only context that
  reaches permission checks, tools, and subagents without being interpreted or
  persisted by core.
- Added durable session interaction modes: default `Interactive` and opt-in
  `Detached { on_ask: Allow|Deny }`, including detached permission handling.
- Documented the library/composition-root model, `RouterBuilder` integration,
  host-owned cloud adapters, and the reference-plugin boundary.

## Decisions and security boundaries

- `leti-core` owns agent mechanics and port traits; hosts own authentication,
  tenancy, credentials, HITL policy, and cloud implementations. A core-purity
  regression test guards both dependency direction and business identifiers.
- Turn extensions are opaque to the engine: never serialized, persisted,
  logged, or given identity semantics.
- Detached mode is never the default. Explicit ruleset `Deny` remains final;
  destructive shell subjects and `web_fetch` stay fail-closed unless an
  explicit allow rule exists. Detached executions remain auditable, including
  when permission mode is `Danger`.

## Validation

- The plan is marked complete across all five phases.
- Focused regression coverage exists for core purity, extension propagation,
  detached permission resolution, hardened egress/destructive-shell handling,
  persistence, routing, and updated documentation contracts.
- The final workspace diff reflects removal of the cloudfs implementation and
  addition of the new engine seams and integration guidance.

## Next steps

- Openlet can build its host-side cloud filesystem and assistant plugin against
  the documented ports and composition root.
- Do not retire the Python `leti-service` until the host integration reaches
  parity.
- AgentWiki publishing was skipped because no AgentWiki CLI or MCP integration
  is available in this workspace.
