// The prompt's meta row, ported from OpenCode's `component/prompt/index.tsx`
// (the agent/model line under the textarea). Shows the active agent name in the
// rail color, then `· model` in muted text. NO provider field — `AgentDto` has
// none and the plan keeps the backend wiring intact. NO variant — that UI was
// dropped. An optional right slot mirrors OpenCode's `props.right` (the editor
// passes nothing for now; Phase 5/6 may fill it).

import { Show } from "solid-js";
import type { JSX } from "@opentui/solid";

import { theme } from "../theme/index.js";

import type { AgentDto } from "../api/types.js";

export interface PromptMetaRowProps {
  /// Active agent for the session, or null before one is selected.
  agent: AgentDto | null;
  /// Model label (agent's model, or a fallback dash when unset).
  model: string;
  /// Rail/accent color, matching the editor's left bar.
  accent: string;
  /// Optional right-aligned content (unused for now; kept for parity).
  right?: JSX.Element;
}

function titlecase(value: string): string {
  return value.length === 0 ? value : value[0]!.toUpperCase() + value.slice(1);
}

export function PromptMetaRow(props: PromptMetaRowProps) {
  const oc = theme.oc;
  return (
    <box flexDirection="row" flexShrink={0} paddingTop={1} gap={1} justifyContent="space-between">
      <box flexDirection="row" gap={1}>
        <Show when={props.agent} fallback={<box height={1} />}>
          {(agent) => (
            <>
              <text fg={props.accent}>{titlecase(agent().display_name)}</text>
              <text fg={oc.textMuted}>·</text>
              <text flexShrink={0} fg={oc.text}>
                {props.model}
              </text>
            </>
          )}
        </Show>
      </box>
      <Show when={props.right}>
        <box flexDirection="row" gap={1} alignItems="center">
          {props.right}
        </box>
      </Show>
    </box>
  );
}
