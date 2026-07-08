import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createEventPump } from "../src/render/event-pump.js";

import type { EventDto } from "../src/api/types.js";

function delta(partId: string, text: string, kind: "text" | "reasoning" = "text"): EventDto {
  return {
    kind: "part_delta",
    session_id: "s1",
    message_id: "m1",
    part_id: partId,
    delta_kind: kind,
    delta: text,
  } as EventDto;
}

describe("createEventPump", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("coalesces same-part deltas into a single merged delta on flush", () => {
    const seen: EventDto[] = [];
    const pump = createEventPump((ev) => seen.push(ev));

    pump.push(delta("p1", "Hel"));
    pump.push(delta("p1", "lo, "));
    pump.push(delta("p1", "world"));
    // Nothing dispatched until the frame timer fires.
    expect(seen).toHaveLength(0);

    vi.advanceTimersByTime(33);
    expect(seen).toHaveLength(1);
    expect(seen[0]).toMatchObject({ kind: "part_delta", part_id: "p1", delta: "Hello, world" });
  });

  it("keeps distinct parts and delta kinds in separate buffers", () => {
    const seen: EventDto[] = [];
    const pump = createEventPump((ev) => seen.push(ev));

    pump.push(delta("p1", "text-a"));
    pump.push(delta("p2", "text-b"));
    pump.push(delta("p1", "reason", "reasoning"));

    vi.advanceTimersByTime(33);
    // Three keys (p1:text, p2:text, p1:reasoning), flushed in insertion order.
    expect(seen).toHaveLength(3);
    expect(seen[0]).toMatchObject({ part_id: "p1", delta_kind: "text", delta: "text-a" });
    expect(seen[1]).toMatchObject({ part_id: "p2", delta_kind: "text", delta: "text-b" });
    expect(seen[2]).toMatchObject({ part_id: "p1", delta_kind: "reasoning", delta: "reason" });
  });

  it("flushes buffered deltas BEFORE a non-delta event so ordering is preserved", () => {
    const seen: EventDto[] = [];
    const pump = createEventPump((ev) => seen.push(ev));

    pump.push(delta("p1", "streamed text"));
    // part_updated finalizes the part (buffer -> text). It MUST land after the
    // delta it finalizes, not race ahead of the still-buffered token.
    const updated = {
      kind: "part_updated",
      session_id: "s1",
      message_id: "m1",
      part_id: "p1",
    } as EventDto;
    pump.push(updated);

    // Both dispatched synchronously on the non-delta push, delta first.
    expect(seen).toHaveLength(2);
    expect(seen[0]).toMatchObject({ kind: "part_delta", delta: "streamed text" });
    expect(seen[1]).toMatchObject({ kind: "part_updated" });
  });

  it("dispose flushes any pending buffer so a closing stream loses no tokens", () => {
    const seen: EventDto[] = [];
    const pump = createEventPump((ev) => seen.push(ev));

    pump.push(delta("p1", "tail"));
    expect(seen).toHaveLength(0);
    pump.dispose();
    expect(seen).toHaveLength(1);
    expect(seen[0]).toMatchObject({ kind: "part_delta", delta: "tail" });
  });

  it("batches high-frequency deltas across frames, not per token", () => {
    const seen: EventDto[] = [];
    const pump = createEventPump((ev) => seen.push(ev));

    // 10 tokens arrive within one frame window → one merged flush.
    for (let i = 0; i < 10; i++) pump.push(delta("p1", String(i)));
    vi.advanceTimersByTime(33);
    expect(seen).toHaveLength(1);
    expect(seen[0]).toMatchObject({ delta: "0123456789" });

    // Next frame's tokens flush separately.
    pump.push(delta("p1", "-next"));
    vi.advanceTimersByTime(33);
    expect(seen).toHaveLength(2);
    expect(seen[1]).toMatchObject({ delta: "-next" });
  });
});
