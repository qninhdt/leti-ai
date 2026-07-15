// Message list — the scrollbox content loop. Renders ordered messages,
// dispatching each to the user or assistant template by role, with
// `marginTop=1` between them (first gets 0, supplied by the route's top
// spacer). Plan-mode banner shows above when active.
//
// <Index>, not <For>: the store hands us a NEW message object every streamed
// token (immutable update). <For> keys by reference, so it would dispose and
// rebuild the whole assistant subtree — including the markdown renderable —
// on every token, which reads as flicker. <Index> keys by position, keeping
// each row's component instance alive and feeding the new value through the
// accessor so children update in place.

import { Index, Show } from "solid-js";

import { theme } from "../theme/index.js";
import { isRuntimeReminderOnly } from "../store/message-hydration.js";
import { MessageUser } from "./message-user.js";
import { MessageAssistant } from "./message-assistant.js";

import type { MessageView } from "../store/index.js";

export interface MessageListProps {
  messages: MessageView[];
  /// Agent accent color for user bars + assistant footer glyph.
  accent: string;
  /// Model label for the assistant footer.
  model: string;
  planMode?: boolean;
}

export function MessageList(props: MessageListProps) {
  const oc = theme.oc;
  return (
    <box flexDirection="column">
      <Show when={props.planMode}>
        <box flexDirection="row" gap={1} marginTop={1}>
          <text fg={oc.accent}>▣ Plan mode</text>
          <text fg={oc.textMuted}>· read-only until ExitPlanMode</text>
        </box>
      </Show>
      <Index each={props.messages.filter((message) => !isRuntimeReminderOnly(message))}>
        {(message) => (
          <box marginTop={1}>
            <Show
              when={message().role === "user"}
              fallback={
                <MessageAssistant
                  message={message()}
                  accent={props.accent}
                  model={props.model}
                />
              }
            >
              <MessageUser message={message()} accent={props.accent} />
            </Show>
          </box>
        )}
      </Index>
    </box>
  );
}
