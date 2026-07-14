// Pure parser for a `subagent_task` tool call → the fields the inline task
// block renders. Split from the tsx renderer so it's unit-testable without the
// Solid JSX runtime (mirrors tool-diff-parse.ts / tool-todo-parse.ts).
//
// The `subagent_task` tool's args carry `{ subagent_type, objective, background }`;
// its result (when sync) serializes `{ task_id, status, output, cost_usd }`.
// Live status/cost for a background task come from the `subagents` store slice
// (keyed by task_id), NOT from this parse — this only extracts what the tool
// call itself records so the block can render before any SSE frame lands.

export interface SubagentCall {
  /// Agent slug being spawned (from args.subagent_type).
  agent: string;
  /// The objective text (args.objective), for the block subtitle.
  objective: string;
  /// Whether the call requested background execution.
  background: boolean;
  /// task_id from the tool result, if the call already returned one.
  taskId?: string;
  /// Terminal status string from a sync result (running/finished/…), if present.
  status?: string;
  /// Cost string from a sync result, if present.
  cost?: string;
}

function tryParse(raw: unknown): Record<string, unknown> | null {
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

function str(o: Record<string, unknown>, key: string): string | undefined {
  const v = o[key];
  return typeof v === "string" ? v : undefined;
}

/// Extract the subagent call shape from a `subagent_task` tool part's args +
/// (optional) result. Returns null when args don't carry a `subagent_type`
/// (not a well-formed subagent call). Tolerant of a missing/streaming result.
export function parseSubagentCall(args: unknown, result?: unknown): SubagentCall | null {
  const a = tryParse(args);
  if (!a) return null;
  const agent = str(a, "subagent_type");
  if (!agent) return null;
  const objective = str(a, "objective") ?? "";
  const background = a.background === true;

  const call: SubagentCall = { agent, objective, background };

  const r = tryParse(result);
  if (r) {
    call.taskId = str(r, "task_id");
    call.status = str(r, "status");
    call.cost = str(r, "cost_usd");
  }
  return call;
}
