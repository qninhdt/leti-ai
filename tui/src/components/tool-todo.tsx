// Inline transcript row for a `todo` tool call. The full checklist now lives in
// the sidebar (mirrors OpenCode, which surfaces the plan there); here we render
// only a compact one-liner so the transcript isn't dominated by the plan on
// every update. Reads args (not the result count) so it shows while settling.

import { theme } from "../theme/index.js";
import { parseTodos } from "./tool-todo-parse.js";

import type { PartView } from "../store/index.js";

export interface ToolTodoProps {
  part: PartView;
}

export function ToolTodo(props: ToolTodoProps) {
  const oc = theme.oc;
  const items = () => parseTodos(props.part.tool_args);
  const summary = () => {
    const list = items();
    if (list.length === 0) return "update todos";
    const done = list.filter((t) => t.status === "completed").length;
    return `updated todos · ${done}/${list.length} done`;
  };

  return (
    <box paddingLeft={3} flexDirection="row" gap={1}>
      <text fg={oc.textMuted}>✱</text>
      <text fg={oc.textMuted} wrapMode="none">
        {summary()}
      </text>
    </box>
  );
}
