// Coalesces the per-token `part_delta` flood into ~30fps batches before it
// reaches the store. The provider streams one delta per token (50-100/s); the
// store's `applyEvent` allocates a new state on each, which re-runs the
// assistant markdown parse + syntax-highlight and a scroll reassert PER TOKEN.
// At 100 tok/s that is 100 full re-renders a second — read as stutter (lag) and,
// because `conceal` shows/hides not-yet-closed markdown markers on every token,
// as flicker. Accumulating deltas keyed by part+kind and flushing one merged
// delta on a frame timer collapses N per-token renders into ~30/s.
//
// Ordering invariant: a non-delta event (notably `part_updated`, which moves
// buffer→text on finalize) MUST land after every delta already accumulated for
// that part. So any non-delta event flushes the pending buffer first, then
// dispatches itself. Deltas are flushed in insertion order (Map preserves it),
// which is the order they streamed in.

import type { EventDto } from "../api/types.js";

/// Frame budget between delta flushes. ~30fps: smooth to the eye, but ~3x fewer
/// markdown reparses than the raw token rate at 100 tok/s.
const FLUSH_MS = 33;

interface PendingDelta {
  session_id: string;
  message_id: string;
  part_id: string;
  delta_kind: "text" | "reasoning" | "tool_args";
  delta: string;
}

export interface EventPump {
  /// Feed one SSE event. Text/reasoning/tool-arg deltas are buffered and
  /// flushed on the frame timer; every other event flushes first, then applies.
  push(ev: EventDto): void;
  /// Flush any buffered deltas and cancel the timer. Call on teardown so a
  /// pending flush never fires after the stream closes.
  dispose(): void;
}

/// Build a coalescing pump that forwards merged events to `apply` (the store's
/// applyEvent). `now`/`schedule` are injectable so tests can drive time.
export function createEventPump(apply: (ev: EventDto) => void): EventPump {
  // Keyed by message+part+kind so two parts (or text vs reasoning of one part)
  // accumulate independently. Map iteration order = insertion order = stream
  // order, so flushing in-order preserves how the tokens arrived.
  const pending = new Map<string, PendingDelta>();
  let timer: ReturnType<typeof setTimeout> | null = null;

  const flush = (): void => {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
    if (pending.size === 0) return;
    // Snapshot then clear first: `apply` runs synchronous store subscribers, and
    // clearing up front keeps the buffer consistent even if one throws.
    const batch = Array.from(pending.values());
    pending.clear();
    for (const p of batch) {
      apply({
        kind: "part_delta",
        session_id: p.session_id,
        message_id: p.message_id,
        part_id: p.part_id,
        delta_kind: p.delta_kind,
        delta: p.delta,
      });
    }
  };

  return {
    push(ev) {
      if (ev.kind === "part_delta") {
        const key = `${ev.message_id}:${ev.part_id}:${ev.delta_kind}`;
        const acc = pending.get(key);
        if (acc) acc.delta += ev.delta;
        else
          pending.set(key, {
            session_id: ev.session_id,
            message_id: ev.message_id,
            part_id: ev.part_id,
            delta_kind: ev.delta_kind,
            delta: ev.delta,
          });
        if (timer === null) timer = setTimeout(flush, FLUSH_MS);
        return;
      }
      // Non-delta event: drain buffered deltas so this event lands after them,
      // then dispatch it.
      flush();
      apply(ev);
    },
    dispose() {
      flush();
    },
  };
}
