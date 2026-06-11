// Tool visual classification, ported from OpenCode's session part-rendering
// (`routes/session/index.tsx`). Maps a tool name to its inline icon and decides
// whether it renders as a one-line InlineTool or an expandable BlockTool, plus
// the collapse line-limit for block output. Icons mirror OpenCode exactly:
// Shell `$`, Read `→`, Write/Edit `←`, Glob/Grep `✱`, WebFetch `%`,
// WebSearch `◈`, Task `#`, generic `⚙`.

export type ToolTemplate = "inline" | "block";

export interface ToolVisual {
  icon: string;
  template: ToolTemplate;
  /// Block-output collapse limit in lines (only meaningful for block tools).
  collapseLines: number;
}

const GENERIC_COLLAPSE = 3;
const SHELL_COLLAPSE = 10;

// Keyed by normalized (lowercased) tool name. Names not listed fall back to the
// generic `⚙` inline treatment. Shell/Read/Write/Edit and the search tools are
// the ones OpenCode gives distinct icons + block bodies.
const TOOL_VISUALS: Record<string, ToolVisual> = {
  shell: { icon: "$", template: "block", collapseLines: SHELL_COLLAPSE },
  bash: { icon: "$", template: "block", collapseLines: SHELL_COLLAPSE },
  read: { icon: "→", template: "inline", collapseLines: GENERIC_COLLAPSE },
  write: { icon: "←", template: "block", collapseLines: GENERIC_COLLAPSE },
  edit: { icon: "←", template: "block", collapseLines: GENERIC_COLLAPSE },
  glob: { icon: "✱", template: "inline", collapseLines: GENERIC_COLLAPSE },
  grep: { icon: "✱", template: "inline", collapseLines: GENERIC_COLLAPSE },
  webfetch: { icon: "%", template: "inline", collapseLines: GENERIC_COLLAPSE },
  websearch: { icon: "◈", template: "inline", collapseLines: GENERIC_COLLAPSE },
  task: { icon: "#", template: "inline", collapseLines: GENERIC_COLLAPSE },
};

const GENERIC_VISUAL: ToolVisual = {
  icon: "⚙",
  template: "inline",
  collapseLines: GENERIC_COLLAPSE,
};

/// Resolve a tool's visual treatment by name (case-insensitive). Unknown tools
/// get the generic inline `⚙` line.
export function toolVisual(toolName: string | undefined): ToolVisual {
  if (!toolName) return GENERIC_VISUAL;
  return TOOL_VISUALS[toolName.toLowerCase()] ?? GENERIC_VISUAL;
}

/// Clamp multi-line tool output to `limit` lines AND a char budget, returning
/// the kept text and whether anything was dropped (drives the "expand"
/// affordance). The char cap matters because a single newline-free blob (e.g.
/// a minified JSON or env dump) would otherwise bypass the line cap and
/// splatter large, possibly secret-bearing output in full.
export function collapseOutput(
  output: string,
  limit: number,
  charCap: number = limit * 200,
): { text: string; overflow: boolean } {
  const lines = output.split("\n");
  let text = output;
  let overflow = false;
  if (lines.length > limit) {
    text = lines.slice(0, limit).join("\n");
    overflow = true;
  }
  if (text.length > charCap) {
    text = text.slice(0, charCap);
    overflow = true;
  }
  return { text, overflow };
}
