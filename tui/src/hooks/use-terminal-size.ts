// Terminal dimensions accessor. Thin re-export of @opentui/solid's
// useTerminalDimensions so layout code has a single import site and the
// engine dependency stays swappable. Returns an Accessor<{width,height}>
// that updates on resize (the renderer re-emits on SIGWINCH).

import { useTerminalDimensions } from "@opentui/solid";
import type { Accessor } from "solid-js";

export interface TerminalSize {
  width: number;
  height: number;
}

/// Width past which the session route shows the sidebar inline rather than
/// as an absolute overlay. Mirrors OpenCode's `wide = width > 120`.
export const WIDE_BREAKPOINT = 120;

export function useTerminalSize(): Accessor<TerminalSize> {
  return useTerminalDimensions();
}

export function isWide(size: TerminalSize): boolean {
  return size.width > WIDE_BREAKPOINT;
}
