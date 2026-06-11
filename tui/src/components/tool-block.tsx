// BlockTool expandable card, ported from OpenCode's `BlockTool`
// (`routes/session/index.tsx:1822`). A left-bar `┃` card (near-invisible bar,
// panel background) with a muted title row, a spinner while the tool runs, and
// the tool output below — collapsed to the tool's line limit with a
// click-to-expand affordance. Output is rendered as plain text; structured
// diff/code embedding is deferred until the backend emits a known diff shape on
// tool_result (the DTO's tool_result is currently untyped — no phantom binding).

import { createMemo, createSignal, Show } from "solid-js";
import "opentui-spinner/solid";

import { theme } from "../theme/index.js";
import { SPLIT_BORDER } from "../utils/border-chars.js";
import { toolVisual, collapseOutput } from "./tool-visuals.js";

import type { PartView } from "../store/index.js";

export interface ToolBlockProps {
  part: PartView;
  /// Title line (e.g. "# Shell: run tests"). Rendered muted, indented.
  title: string;
  /// Tool output body text (already stringified by the caller).
  output: string;
}

export function ToolBlock(props: ToolBlockProps) {
  const oc = theme.oc;
  const [expanded, setExpanded] = createSignal(false);
  const running = () => props.part.status === "streaming";
  const limit = () => toolVisual(props.part.tool_name).collapseLines;

  const collapsed = createMemo(() => collapseOutput(props.output, limit()));
  const shown = () => (expanded() || !collapsed().overflow ? props.output : collapsed().text);

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
      onMouseUp={() => collapsed().overflow && setExpanded((v) => !v)}
    >
      <Show
        when={running()}
        fallback={
          <text paddingLeft={3} fg={oc.textMuted}>
            {props.title}
          </text>
        }
      >
        <box paddingLeft={3} flexDirection="row" gap={1}>
          <spinner color={oc.textMuted} />
          <text fg={oc.textMuted}>{props.title.replace(/^#\s*/, "")}</text>
        </box>
      </Show>
      <Show when={shown()}>
        <text fg={oc.text}>{shown()}</text>
      </Show>
      <Show when={collapsed().overflow}>
        <text fg={oc.textMuted}>{expanded() ? "Click to collapse" : "Click to expand"}</text>
      </Show>
      <Show when={props.part.status === "errored"}>
        <text fg={oc.error}>errored</text>
      </Show>
    </box>
  );
}
