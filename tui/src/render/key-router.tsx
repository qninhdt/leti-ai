// Single global key dispatcher. Replaces the scattered per-view useInput
// handlers from the Ink build. Precedence: an open overlay consumes keys
// first (Esc pops it), then the active route. Per-surface handlers register
// via `setRouteHandler`; the prompt editor (Phase 3) and dialogs (Phase 5)
// install their own handlers through the same registry so there is one
// authoritative key path and no dead-end views.

import { useKeyboard } from "@opentui/solid";

import { useStore } from "../store/index.js";

import type { ParsedKey } from "@opentui/core";

/// A surface key handler returns true when it has consumed the event so the
/// router stops propagating to lower-precedence surfaces.
export type KeyHandler = (key: ParsedKey) => boolean;

let routeHandler: KeyHandler | null = null;
let overlayHandler: KeyHandler | null = null;

/// The active route registers its handler here (cleared on unmount).
export function setRouteHandler(handler: KeyHandler | null): void {
  routeHandler = handler;
}

/// The top overlay registers its handler here so it gets keys before the route.
export function setOverlayHandler(handler: KeyHandler | null): void {
  overlayHandler = handler;
}

function isEscape(key: ParsedKey): boolean {
  return key.name === "escape";
}

/// Installs the global key dispatcher. Call once from the root component.
export function useKeyRouter(): void {
  // Ctrl+C exit is owned by the engine's exitOnCtrlC (set in mount.ts), which
  // restores the terminal (alt-screen/cooked-mode/mouse) before exiting. We do
  // NOT also handle it here — a raw process.exit would skip that teardown and
  // leave the terminal corrupted. runtime.exit stays for the /quit command path
  // (Phase 3+), which must run the same engine teardown before exiting.
  useKeyboard((key: ParsedKey) => {
    const store = useStore.getState();
    const overlays = store.overlays;
    const top = overlays[overlays.length - 1];

    if (top) {
      if (overlayHandler?.(key)) return;
      if (isEscape(key)) {
        // A permission overlay must be RESOLVED (via permission_resolved by
        // askId), never silently dismissed — popping it would orphan the
        // pending request with no way to re-surface it. Swallow Esc here; the
        // explicit reply/deny wiring lands in Phase 5. Other overlays
        // (pickers, help, plugins, palette) are freely Esc-dismissable.
        if (top.kind !== "permission") store.popOverlay();
        return;
      }
      // Overlay is modal: swallow keys that would otherwise hit the route.
      return;
    }

    routeHandler?.(key);
  });
}
