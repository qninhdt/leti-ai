// Renders the overlay stack atop the active route. Only the TOP entry is shown
// (modal semantics), centered over a dim backdrop. The concrete dialog bodies
// (agent/session pickers, permission, help, plugins, command palette) are
// restyled in Phase 5; here each kind renders a labelled placeholder so the
// layer — backdrop, centering, narrow-width handling — is exercised and the
// key router's Esc-pops-overlay path is reachable.

import { Show } from "solid-js";

import { theme } from "../theme/index.js";
import { useStoreSelector } from "./store-bridge.js";

import type { OverlayEntry } from "../store/index.js";

const TITLES: Record<OverlayEntry["kind"], string> = {
  permission: "Permission required",
  agent_picker: "Agents",
  session_picker: "Sessions",
  help: "Help",
  plugins: "Plugins",
  command_palette: "Commands",
};

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
          <box
            border={["left"]}
            borderColor={oc.borderActive}
            backgroundColor={oc.backgroundPanel}
            paddingLeft={2}
            paddingRight={2}
            paddingTop={1}
            paddingBottom={1}
            minWidth={32}
          >
            <text fg={oc.text}>{TITLES[entry().kind]}</text>
            <text fg={oc.textMuted}>esc to close</text>
          </box>
        </box>
      )}
    </Show>
  );
}
