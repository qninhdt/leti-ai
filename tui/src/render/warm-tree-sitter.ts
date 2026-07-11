// Warms the tree-sitter highlight worker before any assistant text streams.
//
// The rich <markdown> element renders prose through a CodeRenderable with
// filetype="markdown" and streaming=true. opentui keeps streaming smooth by
// double-buffering: while the async highlight for token N runs, it keeps
// showing the already-highlighted buffer from token N-1. That smoothing only
// works once the tree-sitter client is initialized and returning highlights.
//
// The client initializes LAZILY — the first highlightOnce() call awaits worker
// spawn + WASM load. If that cold start happens mid-stream, the first tokens
// fall back to plain setText() every frame (no cached highlights yet), which
// reads as the trailing block flickering plain<->styled on each token. Warming
// the parser at boot moves that one-time cost off the streaming path.

import { getTreeSitterClient } from "@opentui/core";

// Filetypes worth having hot before the first turn: markdown for all prose,
// plus the languages most likely to appear in a streamed fenced code block.
const WARM_FILETYPES = ["markdown", "typescript", "javascript", "rust", "python", "json", "bash"];

let warmed = false;

/// Preload the highlight parsers once, fire-and-forget. Safe to call redundantly
/// (guards against double-warm); failures are swallowed since a cold parser only
/// degrades to plain text, never crashes.
export function warmTreeSitter(): void {
  if (warmed) return;
  warmed = true;
  const client = getTreeSitterClient();
  for (const filetype of WARM_FILETYPES) {
    void client.preloadParser(filetype).catch(() => {
      // A missing/failed parser just means that language renders unstyled.
    });
  }
}
