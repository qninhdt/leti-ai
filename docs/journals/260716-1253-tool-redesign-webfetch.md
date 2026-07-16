---
date: 2026-07-16
topic: tool-redesign-webfetch
plan: plans/260716-0214-tool-redesign-webfetch
---

# Tool Redesign and Web Fetch Completion

## Context

Completed the tool-redesign/web-fetch plan: safer grouped file edits, durable
todo update events, and a constrained outbound `web_fetch` capability.

## What happened

- Added atomic edit batches so multi-file edits validate before committing,
  avoiding partially applied tool operations.
- Added the `todo.updated` event path and TUI handling so todo state stays
  synchronized after tool execution.
- Added `web_fetch` with DNS/IP pinning, proxy disabled, and Ask-gated egress
  permissions. Requests cannot follow DNS rebinding to an unvalidated address.

## Reflection

The shared tool surface now favors explicit, observable state transitions over
best-effort mutations. The web-fetch boundary keeps network access narrow by
making each outbound request both permissioned and address-validated.

## Decisions

- Keep edit batches atomic; callers receive failure rather than a partial
  repository mutation.
- Use `todo.updated` as the explicit synchronization contract across core,
  protocol, plugins, and TUI.
- Pin approved resolved IPs for web fetches, bypass proxies, and require Ask
  egress approval; this is intentionally stricter than a general HTTP client.

## Validation

- Focused Rust tool, protocol, plugin-registration, server integration, and
  TUI event/parser tests passed.
- Formatting, type/build checks, and review confirmed the changed contracts and
  existing tool workflows remain compatible.

## Next

- No commit was created: committing was not authorized.
- AgentWiki publishing was skipped because no local AgentWiki CLI or MCP
  integration was available.
