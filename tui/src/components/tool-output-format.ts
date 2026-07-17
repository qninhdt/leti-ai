// Tool results are persisted as a JSON string of the tool's structured Output
// struct (crates/leti-core/src/tools/builtins/*.rs) because that JSON is
// what the model consumes on the next turn. For display we parse the known
// shapes back into human text so the TUI shows stdout / file content / match
// lists instead of a raw `{"exit_code":0,...}` blob. Unknown shapes and error
// strings pass through unchanged.

type Parsed = Record<string, unknown>;

function tryParse(raw: unknown): Parsed | null {
  if (raw && typeof raw === "object") return raw as Parsed;
  if (typeof raw !== "string") return null;
  const s = raw.trim();
  if (!s.startsWith("{") && !s.startsWith("[")) return null;
  try {
    const v = JSON.parse(s);
    return v && typeof v === "object" ? (v as Parsed) : null;
  } catch {
    return null;
  }
}

function asString(v: unknown): string {
  return typeof v === "string" ? v : "";
}

function asNumber(v: unknown): number {
  return typeof v === "number" ? v : 0;
}

function passthrough(raw: unknown): string {
  return typeof raw === "string" ? raw : "";
}

function formatBash(p: Parsed): string {
  const stdout = asString(p.stdout).replace(/\n+$/, "");
  const stderr = asString(p.stderr).replace(/\n+$/, "");
  const code = asNumber(p.exit_code);
  const chunks: string[] = [];
  if (stdout) chunks.push(stdout);
  if (stderr) chunks.push(stderr);
  let body = chunks.join("\n");
  if (p.timed_out) body = `${body}\n[timed out]`.trim();
  else if (code !== 0) body = `${body}\n[exit ${code}]`.trim();
  return body || (code === 0 ? "(no output)" : `[exit ${code}]`);
}

function formatList(p: Parsed): string {
  const entries = Array.isArray(p.entries) ? p.entries : [];
  if (entries.length === 0) return "(empty)";
  const lines = entries.map((e) => {
    const name = asString((e as Parsed)?.name);
    const isDir = (e as Parsed)?.kind === "dir";
    return `${name}${isDir ? "/" : ""}`;
  });
  if (p.truncated) lines.push("… (truncated)");
  return lines.join("\n");
}

function formatGrep(p: Parsed): string {
  const hits = Array.isArray(p.hits) ? p.hits : [];
  if (hits.length === 0) return "(no matches)";
  const lines = hits.map((h) => {
    const hit = h as Parsed;
    return `${asString(hit?.path)}:${asNumber(hit?.line)}: ${asString(hit?.text)}`;
  });
  if (p.truncated) lines.push("… (truncated)");
  return lines.join("\n");
}

function formatGlob(p: Parsed): string {
  const matches = Array.isArray(p.matches) ? p.matches.map(asString) : [];
  if (matches.length === 0) return "(no matches)";
  if (p.truncated) matches.push("… (truncated)");
  return matches.join("\n");
}

function formatWrite(p: Parsed): string {
  const verb = p.kind === "create" ? "Created" : "Updated";
  return `${verb} ${asString(p.path)} (${asNumber(p.bytes_written)} bytes)`;
}

function formatEdit(p: Parsed): string {
  const n = asNumber(p.replacements);
  return `${asString(p.path)} (${n} replacement${n === 1 ? "" : "s"})`;
}

/// Render a tool's structured JSON result as human-readable text for the TUI.
/// `raw` is the persisted `tool_result` body (a JSON string for successful
/// calls, or a plain error string on failure).
export function formatToolOutput(toolName: string | undefined, raw: unknown): string {
  const parsed = tryParse(raw);
  if (!parsed) return passthrough(raw);

  switch ((toolName ?? "").toLowerCase()) {
    case "bash":
    case "shell":
      return formatBash(parsed);
    case "read":
      return typeof parsed.content === "string" ? parsed.content : passthrough(raw);
    case "list":
      return formatList(parsed);
    case "grep":
      return formatGrep(parsed);
    case "glob":
      return formatGlob(parsed);
    case "write":
      return formatWrite(parsed);
    case "edit":
      return formatEdit(parsed);
    default:
      return passthrough(raw);
  }
}
