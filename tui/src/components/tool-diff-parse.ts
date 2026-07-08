// Parse a tool result body into a FileDiff, or null if it isn't one. The
// edit/write tools serialize their Output (including a `diff` field) to JSON
// which rides through the tool_result text verbatim; here we narrow the
// untyped body back to the diff shape for the ToolDiff renderer.

export type DiffLineKind = "add" | "del" | "ctx";

export interface DiffLine {
  kind: DiffLineKind;
  text: string;
}

export interface DiffHunk {
  old_start: number;
  new_start: number;
  lines: DiffLine[];
}

export interface FileDiff {
  added: number;
  removed: number;
  hunks: DiffHunk[];
  truncated: boolean;
}

function isDiffLine(v: unknown): v is DiffLine {
  if (!v || typeof v !== "object") return false;
  const o = v as Record<string, unknown>;
  return (o.kind === "add" || o.kind === "del" || o.kind === "ctx") && typeof o.text === "string";
}

function parseHunk(v: unknown): DiffHunk | null {
  if (!v || typeof v !== "object") return null;
  const o = v as Record<string, unknown>;
  if (!Array.isArray(o.lines)) return null;
  if (typeof o.old_start !== "number" || typeof o.new_start !== "number") return null;
  const lines: DiffLine[] = [];
  for (const l of o.lines) {
    if (!isDiffLine(l)) return null;
    lines.push(l);
  }
  return { old_start: o.old_start, new_start: o.new_start, lines };
}

function tryParseBody(raw: unknown): Record<string, unknown> | null {
  if (raw && typeof raw === "object") return raw as Record<string, unknown>;
  if (typeof raw !== "string") return null;
  const s = raw.trim();
  if (!s.startsWith("{")) return null;
  try {
    const v = JSON.parse(s);
    return v && typeof v === "object" ? (v as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

// A write "create" has diff: null/absent; edit always has a diff. Returns the
// diff only when it has at least one hunk (an empty diff renders as nothing).
export function parseFileDiff(raw: unknown): FileDiff | null {
  const body = tryParseBody(raw);
  if (!body) return null;
  const diff = body.diff;
  if (!diff || typeof diff !== "object") return null;
  const d = diff as Record<string, unknown>;
  if (!Array.isArray(d.hunks)) return null;
  const hunks: DiffHunk[] = [];
  for (const h of d.hunks) {
    const parsed = parseHunk(h);
    if (parsed) hunks.push(parsed);
  }
  if (hunks.length === 0) return null;
  return {
    added: typeof d.added === "number" ? d.added : 0,
    removed: typeof d.removed === "number" ? d.removed : 0,
    hunks,
    truncated: d.truncated === true,
  };
}
