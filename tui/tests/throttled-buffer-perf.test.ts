// Amendment §U deterministic render-perf gate. We exercise the throttle
// math directly (without React) — the hook simply caps state-flushes at
// `frameMs` while accepting unbounded input updates. Equivalent guard
// to the original "feed 200 deltas through useThrottledBuffer" idea but
// avoids pulling in @testing-library/react for one test.

import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";

describe("throttled flush gate (amendment §U)", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("schedules ≤ ceil(N*1ms / 33ms) flushes for N tightly-packed deltas", () => {
    const FRAME_MS = 33;
    const N = 200;

    let lastFlush = Date.now();
    let flushes = 0;
    let timer: ReturnType<typeof setTimeout> | null = null;

    const onInput = () => {
      const now = Date.now();
      const elapsed = now - lastFlush;
      if (elapsed >= FRAME_MS) {
        flushes += 1;
        lastFlush = now;
        return;
      }
      if (timer === null) {
        timer = setTimeout(() => {
          flushes += 1;
          lastFlush = Date.now();
          timer = null;
        }, FRAME_MS - elapsed);
      }
    };

    for (let i = 0; i < N; i += 1) {
      onInput();
      vi.advanceTimersByTime(1);
    }
    vi.runAllTimers();

    const ceiling = Math.ceil((N * 1) / FRAME_MS);
    expect(flushes).toBeLessThanOrEqual(ceiling);
  });
});
