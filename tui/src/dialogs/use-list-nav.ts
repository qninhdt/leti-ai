// Shared keyboard navigation for overlay list pickers (agents, sessions).
// Returns a reactive cursor index plus a KeyHandler the dialog installs through
// the key router's overlay seam (`setOverlayHandler`). The handler consumes
// Up/Down/Enter (returns true) and lets Escape fall through (returns false) so
// the router's own Esc-pops-overlay path closes the picker.

import { createSignal, type Accessor } from "solid-js";

import type { KeyHandler } from "../render/key-router.js";

export interface ListNav {
  index: Accessor<number>;
  handler: KeyHandler;
}

export function createListNav<T>(items: Accessor<T[]>, onSelect: (item: T) => void): ListNav {
  const [index, setIndex] = createSignal(0);

  const handler: KeyHandler = (key) => {
    if (key.name === "up") {
      setIndex((i) => Math.max(0, i - 1));
      return true;
    }
    if (key.name === "down") {
      setIndex((i) => Math.min(Math.max(0, items().length - 1), i + 1));
      return true;
    }
    if (key.name === "return") {
      const choice = items()[index()];
      if (choice !== undefined) onSelect(choice);
      return true;
    }
    return false;
  };

  return { index, handler };
}
