// Colored diff card for the edit/write tools. Mirrors ToolBlock's chrome (a
// left-bar `┃` card on the panel background, muted title row) but renders the
// structured FileDiff body: `+` lines in green, `-` lines in red, context
// lines muted, under a `+N −M` summary header. Collapses past a line cap with a
// click-to-expand affordance, same as ToolBlock. The diff shape is narrowed
// from the untyped tool_result body by parseFileDiff.

import { createMemo, createSignal, For, Show } from "solid-js";
import "opentui-spinner/solid";

import { theme } from "../theme/index.js";
import { SPLIT_BORDER } from "../utils/border-chars.js";

import type { PartView } from "../store/index.js";
import type { FileDiff, DiffLine } from "./tool-diff-parse.js";

// Rows shown before collapsing. Matches the block tools' generic feel — a diff
// longer than this hides its tail behind a click-to-expand row.
const COLLAPSE_LINES = 24;

export interface ToolDiffProps {
  part: PartView;
  /// Title line (e.g. "# Edit: src/foo.rs"). Rendered muted, indented.
  title: string;
  /// Parsed structured diff (already narrowed by parseFileDiff).
  diff: FileDiff;
}

// Flatten the hunks into a single row list, inserting a muted separator between
// non-adjacent hunks so the reader sees where the file skips.
function flattenLines(diff: FileDiff): DiffLine[] {
  const rows: DiffLine[] = [];
  diff.hunks.forEach((hunk, idx) => {
    if (idx > 0) rows.push({ kind: "ctx", text: "⋯" });
    for (const line of hunk.lines) rows.push(line);
  });
  return rows;
}

export function ToolDiff(props: ToolDiffProps) {
  const oc = theme.oc;
  const [expanded, setExpanded] = createSignal(false);
  const running = () => props.part.status === "streaming";

  const rows = createMemo(() => flattenLines(props.diff));
  const overflow = () => rows().length > COLLAPSE_LINES;
  const shown = () => (expanded() || !overflow() ? rows() : rows().slice(0, COLLAPSE_LINES));

  const header = () => {
    const d = props.diff;
    const trunc = d.truncated ? " (truncated)" : "";
    return `+${d.added} −${d.removed}${trunc}`;
  };

  const rowColor = (kind: DiffLine["kind"]) =>
    kind === "add" ? oc.diffAdd : kind === "del" ? oc.diffDelete : oc.textMuted;
  const prefix = (kind: DiffLine["kind"]) => (kind === "add" ? "+" : kind === "del" ? "-" : " ");

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
      onMouseUp={() => overflow() && setExpanded((v) => !v)}
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
      <text paddingLeft={3} fg={oc.textMuted}>
        {header()}
      </text>
      <box paddingLeft={3} flexDirection="column">
        <For each={shown()}>
          {(line) => (
            <text fg={rowColor(line.kind)} wrapMode="none">
              {prefix(line.kind)}
              {line.text}
            </text>
          )}
        </For>
      </box>
      <Show when={overflow()}>
        <text paddingLeft={3} fg={oc.textMuted}>
          {expanded() ? "Click to collapse" : `Click to expand (${rows().length - COLLAPSE_LINES} more)`}
        </text>
      </Show>
      <Show when={props.part.status === "errored"}>
        <text paddingLeft={3} fg={oc.error}>
          errored
        </text>
      </Show>
    </box>
  );
}
