// Agent picker overlay content, ported from the Ink `AgentPicker` view onto the
// overlay model. Lists agents (by `name` + description — AgentDto has no
// display_name), selecting one creates a session with that agent and makes it
// active, then closes the overlay. Navigation runs through the shared list-nav
// helper wired into the key router's overlay seam; Escape falls through to the
// router's pop. Selected row is highlighted (OpenCode list styling), not boxed.

import { For, Show, onCleanup, onMount } from "solid-js";

import { theme } from "../theme/index.js";
import { useStore } from "../store/index.js";
import { useStoreSelector } from "../render/store-bridge.js";
import { useRuntime } from "../render/app-context.js";
import { setOverlayHandler } from "../render/key-router.js";
import { createListNav } from "./use-list-nav.js";

import type { AgentDto } from "../api/types.js";

export function AgentPickerDialog() {
  const oc = theme.oc;
  const runtime = useRuntime();
  const agents = useStoreSelector((s) => s.agents);

  async function select(agent: AgentDto): Promise<void> {
    const store = useStore.getState();
    store.removeOverlay((e) => e.kind === "agent_picker");
    try {
      const session = await runtime.client.createSession({ agent_id: agent.id });
      const fresh = useStore.getState();
      fresh.setSessions([...Object.values(fresh.sessions), session]);
      fresh.setActiveSession(session.id);
    } catch (err) {
      useStore.getState().setClientError(err instanceof Error ? err.message : String(err));
    }
  }

  const nav = createListNav(agents, (agent) => void select(agent));
  onMount(() => setOverlayHandler(nav.handler));
  onCleanup(() => setOverlayHandler(null));

  return (
    <box flexDirection="column" minWidth={42}>
      <text fg={oc.primary}>Agents</text>
      <Show when={agents().length > 0} fallback={<text fg={oc.textMuted}>(no agents registered)</text>}>
        <For each={agents()}>
          {(agent, i) => (
            <box flexDirection="row">
              <text fg={i() === nav.index() ? oc.accent : oc.text}>
                {i() === nav.index() ? "▸ " : "  "}
                {agent.name}
              </text>
              <Show when={agent.description}>
                {(d) => <text fg={oc.textMuted}> — {d()}</text>}
              </Show>
            </box>
          )}
        </For>
      </Show>
      <text fg={oc.textMuted}>↑↓ select · Enter confirm · Esc cancel</text>
    </box>
  );
}
