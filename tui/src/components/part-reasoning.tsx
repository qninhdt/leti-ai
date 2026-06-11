// Assistant reasoning part, ported from OpenCode's `ReasoningPart`
// (`routes/session/index.tsx:1499`). While streaming: a spinner + "Thinking…".
// Once finalized: a collapsed `+ Thought · N lines` line in warning color that
// toggles open on click to reveal the full reasoning text. There is NO title
// field on a reasoning part — a PartView carries only `reasoning_buffer` free
// text — so the collapsed line derives its signal from the line count, and the
// first non-empty line is used as the title when one exists.

import { createSignal, Show } from "solid-js";
import "opentui-spinner/solid";

import { theme } from "../theme/index.js";

import type { PartView } from "../store/index.js";

export interface PartReasoningProps {
  part: PartView;
}

// Reasoning streams into `reasoning_buffer`; on finalize the store clears it,
// so a complete part's text lives in `text`. Read whichever is populated.
function reasoningContent(part: PartView): string {
  const buffered = part.reasoning_buffer.trim();
  if (buffered) return buffered;
  return (part.text ?? "").trim();
}

function firstLine(text: string): string | undefined {
  const line = text.split("\n").find((l) => l.trim().length > 0);
  if (!line) return undefined;
  const trimmed = line.trim();
  return trimmed.length > 60 ? `${trimmed.slice(0, 57)}…` : trimmed;
}

export function PartReasoning(props: PartReasoningProps) {
  const oc = theme.oc;
  const [expanded, setExpanded] = createSignal(false);

  const content = () => reasoningContent(props.part);
  const streaming = () => props.part.status !== "complete";
  const lineCount = () => content().split("\n").filter((l) => l.trim()).length;
  const title = () => firstLine(content());

  return (
    <Show when={content() || streaming()}>
      <Show
        when={!streaming()}
        fallback={
          <box paddingLeft={3} marginTop={1} flexDirection="row" gap={1}>
            <spinner color={oc.warning} />
            <text fg={oc.textMuted}>Thinking…</text>
          </box>
        }
      >
        <box
          paddingLeft={3}
          marginTop={1}
          flexDirection="column"
          onMouseUp={() => setExpanded((v) => !v)}
        >
          <text fg={oc.warning} wrapMode="none">
            {expanded() ? "− Thought" : "+ Thought"}
            {title() ? `: ${title()}` : ""} · {lineCount()} lines
          </text>
          <Show when={expanded()}>
            <box marginTop={1}>
              <text fg={oc.textMuted}>{content()}</text>
            </box>
          </Show>
        </box>
      </Show>
    </Show>
  );
}
