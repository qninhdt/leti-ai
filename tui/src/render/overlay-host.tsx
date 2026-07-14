// Renders the overlay stack atop the active route. Only the TOP entry is shown
// (modal semantics), centered over a dim backdrop. Dispatches each overlay kind
// to its dialog content: pickers / help / plugins / command palette are plain
// content wrapped here in a panel box. Interactive dialogs
// install their own key handler via the router's overlay seam; pure-content
// dialogs rely on the router's Esc-pops-overlay path.

import { Show, Switch, Match } from "solid-js";

import { theme } from "../theme/index.js";
import { useStoreSelector } from "./store-bridge.js";
import { AgentPickerDialog } from "../dialogs/agent-picker.js";
import { SessionPickerDialog } from "../dialogs/session-picker.js";
import { QuestionDialog } from "../dialogs/question-dialog.js";
import { HelpDialog } from "../dialogs/help-dialog.js";
import { PluginsDialog } from "../dialogs/plugins-dialog.js";
import { CommandPalette } from "../dialogs/command-palette.js";

import type { OverlayEntry } from "../store/index.js";

export function OverlayHost() {
  const oc = theme.oc;
  const overlays = useStoreSelector((s) => s.overlays);
  const top = (): OverlayEntry | undefined => {
    const stack = overlays();
    return stack[stack.length - 1];
  };

  return (
    <Show when={top()}>
      {(entry) => (
        <box
          position="absolute"
          left={0}
          top={0}
          right={0}
          bottom={0}
          backgroundColor="#0000007a"
          justifyContent="center"
          alignItems="center"
        >
          <Switch>
            <Match when={entry().kind === "question"}>
              {/* Owns its own chrome + keys; renders raw without the panel box. */}
              <QuestionDialog
                questionId={(entry() as { kind: "question"; questionId: string }).questionId}
              />
            </Match>
            <Match when={true}>
              <box
                border={["left"]}
                borderColor={oc.borderActive}
                backgroundColor={oc.backgroundPanel}
                paddingLeft={2}
                paddingRight={2}
                paddingTop={1}
                paddingBottom={1}
              >
                <Switch>
                  <Match when={entry().kind === "agent_picker"}>
                    <AgentPickerDialog />
                  </Match>
                  <Match when={entry().kind === "session_picker"}>
                    <SessionPickerDialog />
                  </Match>
                  <Match when={entry().kind === "command_palette"}>
                    <CommandPalette />
                  </Match>
                  <Match when={entry().kind === "plugins"}>
                    <PluginsDialog />
                  </Match>
                  <Match when={entry().kind === "help"}>
                    <HelpDialog />
                  </Match>
                </Switch>
              </box>
            </Match>
          </Switch>
        </box>
      )}
    </Show>
  );
}
