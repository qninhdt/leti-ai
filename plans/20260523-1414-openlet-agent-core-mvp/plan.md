---
title: "Openlet Agent Core MVP"
description: ""
status: in_progress
priority: P2
branch: "main"
tags: []
blockedBy: []
blocks: []
created: "2026-05-23T07:15:03.507Z"
createdBy: "ck:plan"
source: skill
---

# Openlet Agent Core MVP

## Overview

MVP for **openlet-ai** (Team 2): a standalone Rust agent runtime exposing REST + SSE, paired with an Ink/React TUI client. Mirrors opencode's server-client split but written in Rust to absorb claw-code's idioms (sync `ApiClient` facade, generic `ConversationRuntime<C,T>`, ordered `PermissionMode`, `safe_failure_class()`, `MockAnthropicService` parity harness).

**Scope:** general agent + custom-agent framework + ONE reference custom agent (indexer stub). All 6 adapter traits in MVP, local impls only. OpenAI-compat LLM via OpenRouter standard. Code-defined agents (developers, not end users). Agent-owned workspace.

**Out of scope:** Team 1 integration (separate repo `~/projects/openlet`), retriever/RAG, cloud adapters, multi-tenant auth, Sixel image rendering.

**Reference repos** (read at every phase):
- `./temp/opencode` — server-client architecture, route shapes, SSE channel, permission UX, prompt registry
- `./temp/claw-code` — Rust idioms, sync ApiClient, generic runtime, JSONL persistence, mock harness, telemetry split

> **MANDATORY:** before writing each phase plan AND before implementing each phase, spawn parallel exploration subagents on BOTH `./temp/opencode` and `./temp/claw-code` to re-harvest patterns relevant to that phase. The brainstorm summary §17 captured a snapshot — phases must verify it still holds against current source.

**Source artifacts:**
- Brainstorm summary: `plans/agent-core-brainstorm-summary.md`
- Research reports: `research/researcher-rust-crates.md`, `research/researcher-ink-tui.md`

**Timeline:** ~12-14 weeks for 2 devs across the 8 phases below.

## Phases

| Phase | Name | Status |
|-------|------|--------|
| 1 | [Foundation](./phase-01-foundation.md) | Complete |
| 2 | [Storage and Message Model](./phase-02-storage-and-message-model.md) | Complete |
| 3 | [Agent Loop Core](./phase-03-agent-loop-core.md) | Complete |
| 4 | [Tools and Permissions](./phase-04-tools-and-permissions.md) | Complete |
| 4D | [Filesystem Adapter and Agent Invariant](./phase-04d-filesystem-adapter-and-agent-invariant.md) | Pending |
| 5 | [HTTP API and SSE](./phase-05-http-api-and-sse.md) | Complete |
| 6 | [Ink TUI](./phase-06-ink-tui.md) | Pending |
| 7 | [Compaction and Polish](./phase-07-compaction-and-polish.md) | Pending |
| 8 | [Hardening](./phase-08-hardening.md) | Pending |

## Amendments

After plan was drafted, two amendment passes were applied:

1. [amendments-after-red-team.md](./amendments-after-red-team.md) — 27 fixes from red-team review (lettered §A-§U).
2. [amendments-plugin-system.md](./amendments-plugin-system.md) — plugin architecture per `note.md` (Openlet Core ships into Cloud as the same binary; plugins add quota/billing/custom tools/agents without forking).

On any conflict with phase files, amendments win. On conflict between the two amendment docs, plugin-system wins for plugin-related decisions, red-team wins for everything else.

**Research grounding:** `research/researcher-rust-crates.md`, `research/researcher-ink-tui.md`, `research/researcher-clawcode-plugins.md`, `research/researcher-opencode-plugins.md`.

## Decisions Log (divergences from brainstorm, user-confirmed)

| Brainstorm § | Brainstorm decision | Plan decision | Confirmed by user |
|---|---|---|---|
| §13 | Server port 4096 | Server port 8787 | 2026-05-23 |
| §4 | `apps/openlet-server`, `apps/openlet-tui` | `crates/openlet-{core,adapters,protocol,server}` + `tui/` | 2026-05-23 |
| §17.5 | Sync ApiClient facade (claw-code pattern) | End-to-end async loop | 2026-05-23 — revisit if test surface pain emerges in phase-08 |

## Dependencies

<!-- Cross-plan dependencies -->
