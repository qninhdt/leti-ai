// User message, ported from OpenCode's user-message block
// (`routes/session/index.tsx` user branch). A left-bar `┃` panel colored with
// the agent accent, panel background, text in `theme.text`. File-attachment
// badges are Phase 6's concern; here we render the message's text parts only.

import { For, Show } from "solid-js";

import { theme } from "../theme/index.js";
import { SPLIT_BORDER } from "../utils/border-chars.js";

import type { MessageView } from "../store/index.js";

export interface MessageUserProps {
  message: MessageView;
  /// Agent accent color for the left bar (falls back to borderActive).
  accent: string;
}

function partText(text: string | undefined, buffer: string): string {
  return `${text ?? ""}${buffer}`.trim();
}

export function MessageUser(props: MessageUserProps) {
  const oc = theme.oc;
  const textParts = () => props.message.parts.filter((p) => p.kind === "text");

  return (
    <box
      border={["left"]}
      customBorderChars={SPLIT_BORDER}
      borderColor={props.accent}
      backgroundColor={oc.backgroundPanel}
      paddingTop={1}
      paddingBottom={1}
      paddingLeft={2}
      flexDirection="column"
    >
      <For each={textParts()}>
        {(part) => (
          <Show when={partText(part.text, part.buffer)}>
            {(text) => <text fg={oc.text}>{text()}</text>}
          </Show>
        )}
      </For>
    </box>
  );
}
