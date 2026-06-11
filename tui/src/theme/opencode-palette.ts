// OpenCode-style flat color tokens, ported from OpenCode's `oc-2` dark theme
// (packages/ui/src/theme/themes/oc-2.json). Truecolor hex; the engine parses
// these to RGBA. These sit alongside the legacy nested tokens in dark.ts so
// both the new Solid render layer and any not-yet-ported code resolve.

export interface OpenCodePalette {
  background: string;
  backgroundPanel: string;
  backgroundElement: string;
  backgroundMenu: string;
  border: string;
  borderActive: string;
  primary: string;
  secondary: string;
  accent: string;
  text: string;
  textMuted: string;
  warning: string;
  error: string;
  success: string;
  info: string;
  diffAdd: string;
  diffDelete: string;
  /// Default per-agent accent when an agent defines no color of its own.
  agent: string;
}

export const openCodeDark: OpenCodePalette = {
  background: "#1c1c1c",
  backgroundPanel: "#232323",
  backgroundElement: "#282828",
  backgroundMenu: "#2d2d2d",
  border: "#282828",
  borderActive: "#fab283",
  primary: "#fab283",
  secondary: "#edb2f1",
  accent: "#8cb0ff",
  text: "#ededed",
  textMuted: "#a0a0a0",
  warning: "#fcd53a",
  error: "#fc533a",
  success: "#12c905",
  info: "#edb2f1",
  diffAdd: "#c8ffc4",
  diffDelete: "#fc533a",
  agent: "#fab283",
};
