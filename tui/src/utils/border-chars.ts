// Custom border-character sets for the prompt editor's OpenCode-style frame.
// The engine's `customBorderChars` prop wants a full BorderCharacters record,
// so each set fills all 11 slots even when most are blank. Mirrors OpenCode's
// `component/border.ts` (EmptyBorder / SplitBorder) plus the prompt's `╹`/`▀`
// shelf caps from `component/prompt/index.tsx`.

import type { BorderCharacters } from "@opentui/core";

// Every slot blank except a single space horizontal — the neutral base the
// other sets override. A blank corner/edge renders nothing, so a box can show
// exactly one decorated side without the engine drawing a full frame.
export const EMPTY_BORDER: BorderCharacters = {
  topLeft: "",
  topRight: "",
  bottomLeft: "",
  bottomRight: "",
  horizontal: " ",
  vertical: "",
  topT: "",
  bottomT: "",
  leftT: "",
  rightT: "",
  cross: "",
};

// Left bar of the editor body: a solid `┃` rail, capped at the bottom with `╹`
// so the rail visually terminates into the shelf below it.
export const PROMPT_BODY_BORDER: BorderCharacters = {
  ...EMPTY_BORDER,
  vertical: "┃",
  bottomLeft: "╹",
};

// The 1-row shelf directly under the body: its left edge is the `╹` cap glyph
// (continuing the rail's terminus) while the row itself stays empty.
export const PROMPT_SHELF_CAP_BORDER: BorderCharacters = {
  ...EMPTY_BORDER,
  vertical: "╹",
};

// The shelf's underline: a `▀` half-block drawn along the bottom edge, giving
// the prompt its grounded baseline the way OpenCode renders it.
export const PROMPT_SHELF_UNDERLINE_BORDER: BorderCharacters = {
  ...EMPTY_BORDER,
  horizontal: "▀",
};

// Plain `┃` left rail with no cap — OpenCode's SplitBorder, used by block-tool
// output cards (`borderColor` is set near-invisible at the call site).
export const SPLIT_BORDER: BorderCharacters = {
  ...EMPTY_BORDER,
  vertical: "┃",
};
