// Boots the app: parallel-loads agents/plugins/sessions into the store, then
// opens the SSE stream and routes frames to applyEvent. The SSE connection
// re-opens when the active session changes (Solid createEffect tracks the
// accessor), and closes on cleanup.

import { createEffect, onCleanup } from "solid-js";

import { connectSse } from "../api/sse.js";
import { useStore } from "../store/index.js";
import { useStoreSelector } from "./store-bridge.js";
import { useRuntime } from "./app-context.js";
import { createHydrationController } from "./hydration-controller.js";
import { createEventPump } from "./event-pump.js";

import type { OpenletClient } from "../api/client.js";

async function loadInitial(client: OpenletClient): Promise<void> {
  const store = useStore.getState();
  const [agents, plugins, sessions] = await Promise.all([
    client.listAgents().catch(() => []),
    client.listPlugins().catch(() => []),
    client.listSessions().catch(() => []),
  ]);
  store.setAgents(agents);
  store.setPlugins(plugins);
  store.setSessions(sessions);
}

/// Runs bootstrap + SSE lifecycle. Call once from the root component.
export function useBootstrap(): void {
  const runtime = useRuntime();
  void loadInitial(runtime.client);

  const hydration = createHydrationController(runtime.client);

  const activeSessionId = useStoreSelector((s) => s.activeSessionId);
  createEffect(() => {
    const sessionId = activeSessionId() ?? undefined;
    // Fetch server-authoritative bodies for the newly active session so its
    // tool calls (name/args/results) render immediately — the SSE stream alone
    // carries only part ids.
    if (sessionId) hydration.refresh(sessionId);

    // Coalesce the per-token part_delta flood to ~30fps before it hits the
    // store; every other event flushes the buffer first, then applies. The
    // pump is per-connection so its timer is torn down with the stream.
    // hydration.onEvent stays on the raw event (it only reacts to
    // session_status/message_created, never deltas) so hydration timing is
    // unchanged.
    const pump = createEventPump((ev) => useStore.getState().applyEvent(ev));

    const sse = connectSse({
      baseUrl: runtime.baseUrl,
      sessionId,
      token: runtime.token,
      onEvent: (ev) => {
        pump.push(ev);
        hydration.onEvent(ev);
      },
      onState: (status, detail) => useStore.getState().setConn(status, detail),
    });
    onCleanup(() => {
      sse.close();
      pump.dispose();
    });
  });
}
