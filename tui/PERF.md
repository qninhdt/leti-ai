# TUI Performance Gates

Performance budget for the SolidJS / `@opentui` renderer.

## Hard gates (CI)

- **Delta batching.** `part_delta` tokens are coalesced into ~30fps
  batches by the event pump (`src/render/event-pump.ts`, `FLUSH_MS = 33`)
  before they reach the store — one merged mutation per frame instead of
  one per token. `tests/event-pump.test.ts` exercises the flush ordering
  (deltas batched in insertion order; every non-delta event flushes the
  pending buffer first, then dispatches itself).
- **Bundle size.** `dist/cli.mjs` must stay under 5MB.
- **Type check.** `npm run typecheck` must pass with no errors.

## Soft targets (manual smoke)

- **First-byte latency.** From `prompt_async` ack → first `part.delta`
  rendered: < 250ms on dev hardware.
- **Frame avg.** Streaming a 50 tok/s response for 5s: render frame
  duration p95 < 33ms.
- **Cold start.** `bun dist/cli.mjs` to first interactive prompt
  drawn: < 600ms on dev hardware.
- **Reconnect.** `kill -9` server during stream → "reconnecting…"
  badge appears within 250ms (first backoff tick) → after server
  restart, SSE resumes with no missed durable events (transient
  `part.delta` may be lost — `part.updated` carries final text).

## Suggested telemetry (post-MVP)

A `PerfSink` trait with two impls (`MemoryPerfSink`, `JsonlPerfSink`)
mirroring claw-code's `telemetry/src/lib.rs` shape, emitting:

- `FrameRendered { duration_ms, dirty_components }`
- `MarkdownFlushed { bytes, blocks }`
- `ToolCardRendered { tool_name, duration_ms }`
- `StreamFirstByte { latency_ms }`
- `StreamComplete { tokens, duration_ms }`
- `SseConnected { attempt }`
- `SseReconnected { attempt, backoff_ms }`

Out of scope for phase-06 MVP.

## How to investigate regressions

1. Reproduce locally with `OPENLET_BASE_URL=http://localhost:8787 bun dist/cli.mjs`.
2. Add a `console.error("frame", performance.now())` in the suspected
   component; look for missing ~33ms gaps.
3. Check that the event pump (`src/render/event-pump.ts`) is batching
   `part_delta` on the frame timer and that `applyEvent`
   (`src/store/apply-event.ts`, the single mutation point) isn't being
   called for transient events that should be ignored.
