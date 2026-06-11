// Parses `@file` mentions out of the prompt buffer. Two jobs:
//   - activeMention(buffer, cursor): the `@query` token the cursor is currently
//     inside/at the end of, used to drive the autocomplete popup as the user
//     types. Returns the query text and the token's [start, end) span so the
//     editor can replace it on accept.
//   - allMentions(buffer): every `@path` token in the buffer, used at submit
//     time to resolve + embed file content.
// Absolute-path mentions (`@/...`, `@C:\...`) are rejected here as
// defense-in-depth — the server also rejects them — so they never trigger a
// request and are treated as plain text.

export interface MentionSpan {
  /// The path after the `@` (e.g. "src/app.tsx").
  path: string;
  /// Index of the `@` in the buffer.
  start: number;
  /// Index just past the last path char.
  end: number;
}

// A mention path is the run of non-whitespace, non-`@` chars after `@`. We stop
// at whitespace so "@a @b" yields two mentions, and disallow a second `@`.
const MENTION_CHAR = /[^\s@]/;

function isAbsolute(path: string): boolean {
  if (path.startsWith("/") || path.startsWith("\\")) return true;
  // Windows drive-letter (C:\ or C:/).
  return path.length >= 2 && path[1] === ":";
}

/// The `@query` token the cursor sits within, or null. Used for the popup. The
/// cursor must be inside the token (just after the last char counts). An
/// absolute path is not offered for completion.
export function activeMention(buffer: string, cursor: number): MentionSpan | null {
  // Walk left from the cursor to find a `@` not preceded by a path char,
  // stopping at whitespace (which ends any token).
  let i = cursor - 1;
  while (i >= 0 && MENTION_CHAR.test(buffer[i]!)) i--;
  if (i < 0 || buffer[i] !== "@") return null;
  const start = i;
  // `@` must start a token: preceded by start-of-buffer or whitespace.
  if (start > 0 && !/\s/.test(buffer[start - 1]!)) return null;
  // Extend right to the token end (cursor may be mid-token).
  let end = cursor;
  while (end < buffer.length && MENTION_CHAR.test(buffer[end]!)) end++;
  const path = buffer.slice(start + 1, end);
  if (isAbsolute(path)) return null;
  return { path, start, end };
}

/// Every valid `@path` mention in the buffer (absolute paths excluded). Used at
/// submit to resolve content. Empty `@` tokens are skipped.
export function allMentions(buffer: string): MentionSpan[] {
  const out: MentionSpan[] = [];
  const re = /(^|\s)@([^\s@]+)/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(buffer)) !== null) {
    const path = m[2]!;
    if (isAbsolute(path)) continue;
    const start = m.index + m[1]!.length;
    out.push({ path, start, end: start + 1 + path.length });
  }
  return out;
}
