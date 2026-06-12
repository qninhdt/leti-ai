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

export function useTerminalSize(): Accessor<TerminalSize> {
  return useTerminalDimensions();
}
