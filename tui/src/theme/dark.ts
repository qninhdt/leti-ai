// Dark theme. Semantic tokens use a nested shape. Truecolor hex values;
// chalk/Ink falls back to 256/16
// on terminals that don't support truecolor.

import { openCodeDark, type OpenCodePalette } from "./opencode-palette.js";

export interface Theme {
  /// OpenCode-style flat palette (new render layer reads these).
  oc: OpenCodePalette;
  text: {
    primary: string;
    muted: string;
    bold: string;
    italic: string;
    heading: [string, string, string, string, string, string];
  };
  code: {
    inline: string;
    border: string;
    bg: string;
  };
  link: string;
  quote: { text: string; bar: string };
  table: { border: string };
  spinner: { active: string; done: string; failed: string };
  border: { muted: string; active: string };
  diff: { added: string; removed: string; addedBg: string; removedBg: string };
  badge: {
    accent: string;
    warning: string;
    error: string;
    success: string;
  };
  tool: { name: string; ok: string; error: string };
  permission: {
    border: string;
    title: string;
    selected: string;
    danger: string;
  };
}

// Hex matches claw ANSI 256 picks where they exist:
// - border.muted = #a8a8a8  (256:245)
// - code.bg      = #303030  (256:236)
// - diff.removed = #ff5f5f  (256:203)
// - diff.added   = #5fa050  (256:70)
export const dark: Theme = {
  oc: openCodeDark,
  text: {
    primary: "#e0e0e0",
    muted: "#808080",
    bold: "#f5d76e",
    italic: "#d370d3",
    heading: ["#5fd7d7", "#ffffff", "#5f87d7", "#a8a8a8", "#a8a8a8", "#a8a8a8"],
  },
  code: {
    inline: "#5fd75f",
    border: "#a8a8a8",
    bg: "#303030",
  },
  link: "#5f87d7",
  quote: { text: "#a8a8a8", bar: "#a8a8a8" },
  table: { border: "#5fafaf" },
  spinner: { active: "#5f87d7", done: "#5fd75f", failed: "#ff5f5f" },
  border: { muted: "#a8a8a8", active: "#5f87d7" },
  diff: {
    added: "#5fa050",
    removed: "#ff5f5f",
    addedBg: "#0e3a0e",
    removedBg: "#3a0e0e",
  },
  badge: {
    accent: "#5f87d7",
    warning: "#ffaf5f",
    error: "#ff5f5f",
    success: "#5fd75f",
  },
  tool: { name: "#5fd7d7", ok: "#5fd75f", error: "#ff5f5f" },
  permission: {
    border: "#ffaf5f",
    title: "#ffaf5f",
    selected: "#5f87d7",
    danger: "#ff5f5f",
  },
};

export const theme = dark;
