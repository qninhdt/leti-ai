// Pure parser for the `todo` tool's args → checklist items. Split from the
// tsx renderer so it's unit-testable without pulling in Solid's JSX runtime.
// The todo tool replaces the whole list each call; args shape is
// `{ todos: [{ content, status, priority }] }`.

export type TodoStatus = "pending" | "in_progress" | "completed";

export interface TodoItem {
  content: string;
  status: TodoStatus;
  priority?: string;
}

const STATUSES: ReadonlySet<string> = new Set([
  "pending",
  "in_progress",
  "completed",
]);

/// Extract a well-formed todo list from raw tool args. Tolerant: a missing or
/// malformed `todos` yields `[]`, an unknown status coerces to `pending` so a
/// forward-compatible server value still renders rather than dropping the item.
export function parseTodos(args: unknown): TodoItem[] {
  if (!args || typeof args !== "object") return [];
  const todos = (args as { todos?: unknown }).todos;
  if (!Array.isArray(todos)) return [];
  const out: TodoItem[] = [];
  for (const t of todos) {
    if (!t || typeof t !== "object") continue;
    const content = (t as { content?: unknown }).content;
    if (typeof content !== "string") continue;
    const rawStatus = (t as { status?: unknown }).status;
    const status: TodoStatus =
      typeof rawStatus === "string" && STATUSES.has(rawStatus)
        ? (rawStatus as TodoStatus)
        : "pending";
    const priority = (t as { priority?: unknown }).priority;
    out.push({
      content,
      status,
      priority: typeof priority === "string" ? priority : undefined,
    });
  }
  return out;
}
