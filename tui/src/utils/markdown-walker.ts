// Markdown stream buffer.
// Splits an incrementally-growing buffer into "stable" chunks (safe to
// finalize-render with marked) and a "tail" (still growing). Boundary =
// blank line outside a fenced block, OR matching closing fence.

export interface BufferSplit {
  stable: string;
  tail: string;
}

export function findStreamSafeBoundary(buf: string): number | null {
  // Walk lines tracking fence state. Return byte offset of the first
  // safe split point (end-of-line of a blank line outside fence, or
  // end-of-line of a closing fence). Returns null if entire buffer is
  // still inside an open fence with no closing line.
  let i = 0;
  let fence: { token: string; len: number } | null = null;
  while (i < buf.length) {
    const eol = buf.indexOf("\n", i);
    if (eol === -1) break;
    const line = buf.slice(i, eol);
    const trimmed = line.trimStart();

    if (fence === null) {
      const opener = matchFence(trimmed);
      if (opener) {
        fence = opener;
      } else if (trimmed.length === 0) {
        return eol + 1;
      }
    } else {
      // Inside a fence — only a closing fence of equal-or-greater length
      // with the same token closes us out.
      const closer = matchFence(trimmed);
      if (closer && closer.token === fence.token && closer.len >= fence.len) {
        fence = null;
        return eol + 1;
      }
    }
    i = eol + 1;
  }
  return null;
}

function matchFence(line: string): { token: string; len: number } | null {
  let i = 0;
  let token: string | null = null;
  while (i < line.length && (line[i] === "`" || line[i] === "~")) {
    if (token === null) token = line[i]!;
    else if (line[i] !== token) return null;
    i += 1;
  }
  if (token === null || i < 3) return null;
  // Must be followed by either end-of-line or info-string (no extra
  // backticks/tildes inline — pulldown-cmark behavior).
  const rest = line.slice(i);
  if (rest.includes("`") || rest.includes("~")) return null;
  return { token, len: i };
}

export function splitBuffer(buf: string): BufferSplit {
  const boundary = findStreamSafeBoundary(buf);
  if (boundary === null) return { stable: "", tail: buf };
  // Walk additional safe boundaries to consume as much as possible.
  let cut = boundary;
  while (true) {
    const more = findStreamSafeBoundary(buf.slice(cut));
    if (more === null) break;
    cut += more;
  }
  return { stable: buf.slice(0, cut), tail: buf.slice(cut) };
}

// Pre-pass to upgrade outer-fence backtick count when inner content has
// fences of equal-or-greater length. Critical for LLM output that
// embeds code blocks containing code blocks.
// Normalizes nested fences.
export function normalizeNestedFences(md: string): string {
  // Minimal port: detect outermost fence; if inner contains fences of
  // equal length, bump outer to len+1. We iterate until stable.
  let out = md;
  for (let pass = 0; pass < 4; pass += 1) {
    const next = normalizePass(out);
    if (next === out) break;
    out = next;
  }
  return out;
}

function normalizePass(md: string): string {
  const lines = md.split("\n");
  const result: string[] = [];
  let openFence: { token: string; len: number; lineIdx: number } | null = null;
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i] ?? "";
    const m = matchFence(line.trimStart());
    if (openFence === null) {
      if (m) openFence = { token: m.token, len: m.len, lineIdx: i };
      result.push(line);
      continue;
    }
    if (m && m.token === openFence.token && m.len >= openFence.len) {
      openFence = null;
      result.push(line);
      continue;
    }
    if (m && m.token === openFence.token) {
      // Inner fence shorter — fine, render as-is; but if equal that
      // would mis-close. Bump outer to len+1.
      const upgrade = openFence.token.repeat(m.len + 1);
      const original = result[openFence.lineIdx] ?? "";
      const trimmedStart = original.trimStart();
      const indent = original.slice(0, original.length - trimmedStart.length);
      const tail = trimmedStart.replace(/^(`+|~+)/, "");
      result[openFence.lineIdx] = indent + upgrade + tail;
      openFence = { token: openFence.token, len: m.len + 1, lineIdx: openFence.lineIdx };
    }
    result.push(line);
  }
  return result.join("\n");
}
