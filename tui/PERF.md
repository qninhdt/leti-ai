# TUI Performance Gates

Per `phase-06-ink-tui.md` §Non-functional and amendment §U.

## Hard gates (CI)

- **Throttled flush count.** `tests/throttled-buffer-perf.test.ts`
  feeds 200 tightly-packed deltas (1ms apart) through
  `useThrottledBuffer(33)`. Internal flush count must be ≤ ⌈200ms / 33ms⌉
  = 7. Deterministic via `vi.useFakeTimers()`.
- **Bundle size.** `dist/cli.mjs` must stay under 5MB.
- **Type check.** `npm run typecheck` must pass with no errors.

## Soft targets (manual smoke)

- **First-byte latency.** From `prompt_async` ack → first `part.delta`
  rendered: < 250ms on dev hardware.
- **Frame avg.** Streaming a 50 tok/s response for 5s: render frame
  duration p95 < 33ms.
- **Cold start.** `node dist/cli.mjs` to first interactive prompt
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

1. Reproduce locally with `OPENLET_BASE_URL=http://localhost:8787 node dist/cli.mjs`.
2. Add a `console.error("frame", performance.now())` at the top of the
   suspected component; look for missing 33ms gaps.
3. Check that `applyEvent` (single mutation point) isn't being called
   for transient events that should be ignored.
