// Formats a tool call's args into a short, human label — the text shown next
// to the icon (inline tools) or in the card title (block tools). Mirrors
// OpenCode's per-tool title functions: surface the one arg that identifies the
// call (file path, command, pattern) instead of dumping raw JSON. Falls back to
// a compact JSON summary for unknown tools so nothing renders blank.

function asRecord(v: unknown): Record<string, unknown> {
  return v && typeof v === "object" ? (v as Record<string, unknown>) : {};
}

function str(v: unknown): string | undefined {
  return typeof v === "string" && v.length > 0 ? v : undefined;
}

/// Compact JSON for an unknown tool's args, clamped to `max`.
function jsonSummary(value: unknown, max = 60): string {
  if (value === undefined || value === null) return "";
  try {
    const s = typeof value === "string" ? value : JSON.stringify(value);
    return s.length > max ? `${s.slice(0, max - 1)}…` : s;
  } catch {
    return "";
  }
}

/// One-line label for a tool call, keyed by tool name. Returns just the
/// argument detail (no icon, no tool name) — callers prepend those.
export function toolLabel(toolName: string | undefined, args: unknown): string {
  const a = asRecord(args);
  switch ((toolName ?? "").toLowerCase()) {
    case "read":
      return str(a.path) ?? "";
    case "write":
      return str(a.path) ?? "";
    case "edit":
      return str(a.path) ?? "";
    case "list":
      return str(a.path) ?? ".";
    case "glob":
      return str(a.pattern) ?? "";
    case "grep": {
      const pat = str(a.pattern) ?? "";
      const scope = str(a.path_glob);
      return scope ? `${pat} in ${scope}` : pat;
    }
    case "bash":
    case "shell":
      return str(a.command) ?? "";
    case "todo":
      return "update todos";
    default:
      return jsonSummary(args);
  }
}

/// Block-card title for a tool call (e.g. "Wrote hello.txt", "$ ls -l").
/// Prepended with the block's own `# ` stripper in ToolBlock.
export function toolBlockTitle(toolName: string | undefined, args: unknown): string {
  const name = (toolName ?? "tool").toLowerCase();
  const label = toolLabel(toolName, args);
  switch (name) {
    case "write":
      return label ? `Wrote ${label}` : "Write";
    case "edit":
      return label ? `Edited ${label}` : "Edit";
    case "bash":
    case "shell":
      return label ? `$ ${label}` : "Shell";
    default:
      return label ? `${name} ${label}` : name;
  }
}
