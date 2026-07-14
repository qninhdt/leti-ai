// Inline transcript block for a `subagent_task` tool call. Mirrors ToolBlock's
// chrome (left-bar `┃` card on the panel background, muted title row, spinner
// while running) but renders subagent-specific state: the agent slug, live
// status (running/finished/failed), a live output tail, and cost. Live fields
// come from the `subagents` store slice keyed by task_id (updated by the
// spawned/progress/settled SSE frames); the static agent/objective come from
// the tool call's own args via parseSubagentCall.
//
// Promoted-task rule (Validation Session 1): a promoted task's `settled` frame
// carries NO output — the result re-enters the PARENT transcript as an injected
// turn. So a promoted, finished block shows a "result delivered below" note
// instead of duplicating the output here (which it never receives).

import { createMemo, Show } from "solid-js";
import "opentui-spinner/solid";

import { theme } from "../theme/index.js";
import { SPLIT_BORDER } from "../utils/border-chars.js";
import { parseSubagentCall } from "./tool-subagent-parse.js";

import type { PartView, SubagentView } from "../store/index.js";

export interface ToolSubagentBlockProps {
  part: PartView;
  /// Live row from the `subagents` store slice for this call's task_id, if the
  /// SSE frames have arrived. Absent before `subagent.spawned` lands.
  live?: SubagentView;
}

const COLLAPSE_LINES = 12;

export function ToolSubagentBlock(props: ToolSubagentBlockProps) {
  const oc = theme.oc;

  const call = createMemo(() =>
    parseSubagentCall(props.part.tool_args, props.part.tool_result),
  );

  // Effective status: prefer the live SSE row, fall back to the sync result's
  // status, then to the part's own streaming state.
  const status = createMemo<string>(() => {
    if (props.live) return props.live.status;
    const c = call();
    if (c?.status) return c.status;
    return props.part.status === "errored" ? "failed" : "running";
  });

  const running = () => status() === "running";
  const agent = () => props.live?.agent || call()?.agent || "subagent";
  const promoted = () => props.live?.promoted ?? false;

  const outputTail = createMemo(() => {
    const out = props.live?.output ?? "";
    if (!out) return "";
    const lines = out.split("\n");
    return lines.length > COLLAPSE_LINES ? lines.slice(-COLLAPSE_LINES).join("\n") : out;
  });

  const title = () => `# Subagent: @${agent()}`;

  return (
    <box
      border={["left"]}
      customBorderChars={SPLIT_BORDER}
      borderColor={oc.background}
      backgroundColor={oc.backgroundPanel}
      paddingTop={1}
      paddingBottom={1}
      paddingLeft={2}
      marginTop={1}
      gap={1}
      flexDirection="column"
    >
      <Show
        when={running()}
        fallback={
          <text paddingLeft={3} fg={oc.textMuted}>
            {title()} · {status()}
          </text>
        }
      >
        <box paddingLeft={3} flexDirection="row" gap={1}>
          <spinner color={oc.textMuted} />
          <text fg={oc.textMuted}>{title().replace(/^#\s*/, "")}</text>
        </box>
      </Show>

      <Show when={call()?.objective}>
        <text paddingLeft={3} fg={oc.textMuted}>
          {call()?.objective}
        </text>
      </Show>

      {/* Promoted + finished: output was delivered into the parent transcript
          below, not carried on the settled frame — point the reader there
          instead of rendering an empty body. */}
      <Show
        when={promoted() && status() === "finished"}
        fallback={
          <Show when={outputTail()}>
            <text paddingLeft={3} fg={oc.text}>
              {outputTail()}
            </text>
          </Show>
        }
      >
        <text paddingLeft={3} fg={oc.textMuted}>
          ↳ result delivered below
        </text>
      </Show>

      <Show when={status() === "failed"}>
        <text paddingLeft={3} fg={oc.error}>
          failed
        </text>
      </Show>
    </box>
  );
}
