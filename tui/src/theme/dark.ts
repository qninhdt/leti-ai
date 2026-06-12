// Dark theme. The flat OpenCode palette (`oc`) is the only interface
// components read; the old nested shape is removed.

import { openCodeDark, type OpenCodePalette } from "./opencode-palette.js";

export interface Theme {
  oc: OpenCodePalette;
}

export const dark: Theme = {
  oc: openCodeDark,
};

export const theme = dark;
