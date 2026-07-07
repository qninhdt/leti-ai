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

    const sse = connectSse({
      baseUrl: runtime.baseUrl,
      sessionId,
      token: runtime.token,
      onEvent: (ev) => {
        useStore.getState().applyEvent(ev);
        hydration.onEvent(ev);
      },
      onState: (status, detail) => useStore.getState().setConn(status, detail),
    });
    onCleanup(() => sse.close());
  });
}
