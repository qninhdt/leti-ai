// Renders an `ask_user` tool call in the transcript as a readable block —
// question header, the options offered, and (once answered) the selected
// label(s) — instead of dumping raw JSON. Mirrors how OpenCode surfaces an
// interactive prompt. Args carry `{ header, question, options:[{label,
// description}], multi_select }`; the result carries `{ selected:[idx],
// selected_labels:[str] }`. The live selection dialog is a separate overlay
// (QuestionDialog); this is the settled record left in the conversation.

import { For, Show } from "solid-js";

import { theme } from "../theme/index.js";
import { parseAskUser, selectedLabels } from "./tool-ask-user-parse.js";

import type { PartView } from "../store/index.js";

export interface ToolAskUserProps {
  part: PartView;
  /// The tool_result payload for this call, if it has resolved.
  result: unknown;
}

export function ToolAskUser(props: ToolAskUserProps) {
  const oc = theme.oc;
  const spec = () => parseAskUser(props.part.tool_args);
  const chosen = () => new Set(selectedLabels(props.result));
  const answered = () => chosen().size > 0;

  return (
    <box paddingLeft={3} flexDirection="column" gap={1}>
      <box flexDirection="row" gap={1}>
        <text fg={oc.accent}>?</text>
        <text fg={oc.text} wrapMode="none">
          {spec().question || spec().header || "ask_user"}
        </text>
      </box>
      <Show when={spec().options.length > 0}>
        <box flexDirection="column" paddingLeft={2}>
          <For each={spec().options}>
            {(opt) => {
              const picked = () => chosen().has(opt.label);
              return (
                <box flexDirection="row" gap={1}>
                  <text fg={picked() ? oc.accent : oc.textMuted}>
                    {answered() ? (picked() ? "●" : "○") : "·"}
                  </text>
                  <text fg={picked() ? oc.text : oc.textMuted} wrapMode="none">
                    {opt.label}
                    {opt.description ? ` — ${opt.description}` : ""}
                  </text>
                </box>
              );
            }}
          </For>
        </box>
      </Show>
      <Show when={!answered()}>
        <text fg={oc.textMuted}>waiting for answer…</text>
      </Show>
    </box>
  );
}
