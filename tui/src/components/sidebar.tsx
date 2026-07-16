// Session sidebar (width 42 when terminal is wide). Shows session/agent/cost
// summary plus the active session's TODO checklist (mirrors OpenCode, which
// surfaces the plan in the sidebar rather than inline in the transcript). Uses
// fine-grained selectors to only re-render on relevant changes.

import { createMemo, For, Show } from "solid-js";

import { theme } from "../theme/index.js";
import { useStoreSelector } from "../render/store-bridge.js";
import { shortId } from "../utils/format.js";
import { parseTodos, type TodoItem, type TodoStatus } from "./tool-todo-parse.js";

import type { MessageView } from "../store/index.js";
import type { TodoItemDto } from "../api/types.js";

const EMPTY_MESSAGES: MessageView[] = [];

// Glyph per status — matches the inline renderer that used to own this list.
const GLYPH: Record<TodoStatus, string> = {
  pending: "☐",
  in_progress: "◐",
  completed: "☑",
};

// Scan a session's messages newest-first for the most recent `todo` tool call.
// The todo tool replaces the whole list each call, so the latest call IS the
// current plan; earlier calls are superseded.
function latestTodos(messages: MessageView[]): TodoItem[] {
  for (let m = messages.length - 1; m >= 0; m--) {
    const parts = messages[m]!.parts;
    for (let p = parts.length - 1; p >= 0; p--) {
      const part = parts[p]!;
      if (part.kind === "tool_call" && (part.tool_name ?? "").toLowerCase() === "todo") {
        const items = parseTodos(part.tool_args);
        if (items.length > 0) return items;
      }
    }
  }
  return [];
}

export function Sidebar() {
  const oc = theme.oc;
  const activeSessionId = useStoreSelector((s) => s.activeSessionId);
  const sessions = useStoreSelector((s) => s.sessions);
  const agents = useStoreSelector((s) => s.agents);
  const messages = useStoreSelector((s) => {
    const id = s.activeSessionId;
    return id ? s.messages[id] ?? EMPTY_MESSAGES : EMPTY_MESSAGES;
  });
  const liveTodos = useStoreSelector((s) => {
    const id = s.activeSessionId;
    return id ? s.todos[id] : undefined;
  });

  const session = createMemo(() => {
    const id = activeSessionId();
    return id ? sessions()[id] ?? null : null;
  });
  const agent = createMemo(() => {
    const s = session();
    return s ? agents().find((a) => a.id === s.agent_id) ?? null : null;
  });
  // A durable `todo_updated` snapshot wins, including `[]` (which explicitly
  // clears the checklist). Before the first live event, fall back to the last
  // hydrated todo tool call for historical sessions.
  const todos = createMemo((): TodoItem[] => liveTodos() ?? latestTodos(messages()));

  const color = (status: TodoStatus): string => {
    switch (status) {
      case "in_progress":
        return oc.accent;
      case "completed":
        return oc.textMuted;
      default:
        return oc.text;
    }
  };

  return (
    <box flexDirection="column" width={42} paddingLeft={2} paddingTop={1} gap={1}>
      <text fg={oc.textMuted}>SESSION</text>
      <Show when={session()} fallback={<text fg={oc.textMuted}>no active session</text>}>
        {(s) => (
          <box flexDirection="column">
            <text fg={oc.text}>{shortId(s().id)}</text>
            <text fg={oc.textMuted}>{s().status}</text>
            <text fg={oc.textMuted}>{s().permission_mode}</text>
            <Show when={agent()}>
              {(a) => (
                <box flexDirection="column" marginTop={1}>
                  <text fg={oc.text}>{a().display_name}</text>
                  <Show when={a().model}>
                    {(m) => <text fg={oc.textMuted}>{m()}</text>}
                  </Show>
                </box>
              )}
            </Show>
            <text fg={oc.warning}>${s().cost_decimal_str}</text>
          </box>
        )}
      </Show>
      <Show when={todos().length > 0}>
        <box flexDirection="column" marginTop={1} gap={1}>
          <text fg={oc.textMuted}>TODO</text>
          <box flexDirection="column">
            <For each={todos()}>
              {(item) => (
                <box flexDirection="row" gap={1}>
                  <text fg={color(item.status)}>{GLYPH[item.status] ?? "☐"}</text>
                  <text fg={color(item.status)}>{item.content}</text>
                </box>
              )}
            </For>
          </box>
        </box>
      </Show>
    </box>
  );
}
