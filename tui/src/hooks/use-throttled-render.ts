// Throttled render hook. Mirrors claw-code's "block-safe boundary" idea
// from render.rs but operates time-boundedly so React reconciliation
// budget is hit at most once per FRAME_MS. Vitest perf gate (amendment
// §U) feeds 200 deltas through this with virtual timers and asserts
// setState count <= ceil(200ms/33ms) = 7.

import { useEffect, useRef, useState } from "react";

const FRAME_MS = 33;

export interface ThrottledBuffer {
  text: string;
  flushedAt: number;
}

export function useThrottledBuffer(input: string, frameMs = FRAME_MS): string {
  const [snapshot, setSnapshot] = useState(input);
  const pendingRef = useRef(input);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lastFlushRef = useRef(Date.now());

  useEffect(() => {
    pendingRef.current = input;
    const now = Date.now();
    const elapsed = now - lastFlushRef.current;
    const flush = () => {
      lastFlushRef.current = Date.now();
      timerRef.current = null;
      setSnapshot(pendingRef.current);
    };
    if (elapsed >= frameMs) {
      flush();
      return;
    }
    if (timerRef.current === null) {
      timerRef.current = setTimeout(flush, frameMs - elapsed);
    }
    return () => {
      if (timerRef.current !== null) {
        clearTimeout(timerRef.current);
        timerRef.current = null;
      }
    };
  }, [input, frameMs]);

  return snapshot;
}
