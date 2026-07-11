// Interrupt-arming state for the prompt editor. Esc while a turn streams arms
// the interrupt; a second Esc within the reset window aborts the running turn.
// Owns the interrupt counter, its reset timer, and the effect that clears the
// arm when streaming stops.

import { createEffect, createSignal, on, onCleanup, type Accessor } from "solid-js";

import type { AppRuntime } from "../render/app-context.js";

const INTERRUPT_RESET_MS = 5000;

export interface InterruptArmDeps {
  activeSessionId: Accessor<string | null>;
  streaming: Accessor<boolean>;
  runtime: AppRuntime;
}

export interface InterruptArm {
  interruptCount: Accessor<number>;
  armInterrupt: () => void;
  resetInterrupt: () => void;
}

export function createInterruptArm(deps: InterruptArmDeps): InterruptArm {
  const { activeSessionId, streaming, runtime } = deps;
  const [interruptCount, setInterruptCount] = createSignal(0);
  let interruptTimer: ReturnType<typeof setTimeout> | undefined;

  createEffect(
    on(
      streaming,
      (s) => {
        if (!s) setInterruptCount(0);
      },
      { defer: true },
    ),
  );

  onCleanup(() => {
    if (interruptTimer) clearTimeout(interruptTimer);
  });

  function armInterrupt(): void {
    const id = activeSessionId();
    if (!streaming() || !id) return;
    const next = interruptCount() + 1;
    setInterruptCount(next);
    if (interruptTimer) clearTimeout(interruptTimer);
    interruptTimer = setTimeout(() => setInterruptCount(0), INTERRUPT_RESET_MS);
    if (next >= 2) {
      void runtime.client.abort(id).catch(() => {});
      setInterruptCount(0);
    }
  }

  function resetInterrupt(): void {
    setInterruptCount(0);
  }

  return { interruptCount, armInterrupt, resetInterrupt };
}
