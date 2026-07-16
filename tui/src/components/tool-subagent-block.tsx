// Inline transcript block for a `subagent_task` tool call. Mirrors ToolBlock's
// chrome (left-bar `┃` card on the panel background, muted title row, spinner
// while running) but renders subagent-specific state: the agent slug, live
// status (running/finished/failed), a live output tail, and cost. Live fields
// come from the `subagents` store slice keyed by task_id (updated by the
// spawned/progress/settled SSE frames); the static agent/objective come from
// the tool call's own args via parseSubagentCall.
//

import { createMemo, createSignal, Show } from "solid-js";
import "opentui-spinner/solid";
import type { BoxRenderable, KeyEvent, MouseEvent } from "@opentui/core";

import { theme } from "../theme/index.js";
import { useStore } from "../store/index.js";
import { useRuntime } from "../render/app-context.js";
import { SPLIT_BORDER } from "../utils/border-chars.js";
import { parseSubagentCall } from "./tool-subagent-parse.js";

import type { PartView, SubagentView } from "../store/index.js";

export interface ToolSubagentBlockProps {
  part: PartView;
  /// Live row from the `subagents` store slice for this call's task_id, if the
  /// SSE frames have arrived. Absent before `subagent.spawned` lands.
  live?: SubagentView;
}

export function ToolSubagentBlock(props: ToolSubagentBlockProps) {
  const oc = theme.oc;
  const runtime = useRuntime();
  let card: BoxRenderable | undefined;
  const [backgrounding, setBackgrounding] = createSignal(false);

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
  const mode = () => (props.live?.background ?? call()?.background ? "background" : "foreground");

  const title = () => `# Subagent: @${agent()}`;
  const childSessionId = () => props.live?.child_session_id;
  const taskId = () => props.live?.task_id ?? call()?.taskId;
  const openChild = () => {
    const id = childSessionId();
    if (id) useStore.getState().setActiveSession(id);
  };
  const onCardClick = (event: MouseEvent) => {
    event.stopPropagation();
    card?.focus();
    openChild();
  };
  const onCardKey = (event: KeyEvent) => {
    if (event.name === "return" || event.name === "enter" || event.name === "space") {
      event.preventDefault();
      openChild();
    }
  };
  const background = async (event: MouseEvent) => {
    event.stopPropagation();
    const id = taskId();
    const parent = props.live?.parent_session_id;
    if (!id || !parent || backgrounding()) return;
    setBackgrounding(true);
    try {
      await runtime.client.backgroundTask(parent, id);
      useStore.getState().setNotice("Subagent continues in background");
    } catch (error) {
      useStore.getState().setClientError(error instanceof Error ? error.message : String(error));
    } finally {
      setBackgrounding(false);
    }
  };

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
      onMouseUp={onCardClick}
      onKeyDown={onCardKey}
      ref={(node: BoxRenderable) => {
        // Spawn is asynchronous, so make the stable card focusable before
        // its child id arrives; Enter simply no-ops until navigation exists.
        node.focusable = true;
        // Keep the focus target stable even when a nested text node received
        // the mouse event.
        card = node;
      }}
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

      <Show when={props.live?.current_activity}>
        <text paddingLeft={3} fg={oc.textMuted}>
          {props.live?.current_activity}
        </text>
      </Show>

      <text paddingLeft={3} fg={oc.textMuted}>
        {mode()}{props.live?.cost ? ` · $${props.live.cost}` : ""}
      </text>

      <Show when={running() && mode() === "foreground" && taskId() && props.live?.parent_session_id}>
        <text paddingLeft={3} fg={oc.primary} onMouseUp={background}>
          {backgrounding() ? "backgrounding…" : "continue in background"}
        </text>
      </Show>

      <Show when={status() === "failed"}>
        <text paddingLeft={3} fg={oc.error}>
          failed
        </text>
      </Show>

      <Show when={status() === "interrupted"}>
        <text paddingLeft={3} fg={oc.warning}>
          interrupted — continue this child from its session
        </text>
      </Show>
    </box>
  );
}
