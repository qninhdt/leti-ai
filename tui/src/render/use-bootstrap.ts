// Boots the app: parallel-loads agents/plugins/sessions into the store, then
// opens the SSE stream and routes frames to applyEvent. Ports the old Ink
// app.tsx useEffect — the SSE connection re-opens when the active session
// changes (Solid createEffect tracks the accessor), and closes on cleanup.

import { createEffect, onCleanup } from "solid-js";

import { connectSse } from "../api/sse.js";
import { useStore } from "../store/index.js";
import { useStoreSelector } from "./store-bridge.js";
import { useRuntime } from "./app-context.js";

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

  const activeSessionId = useStoreSelector((s) => s.activeSessionId);
  createEffect(() => {
    const sse = connectSse({
      baseUrl: runtime.baseUrl,
      sessionId: activeSessionId() ?? undefined,
      token: runtime.token,
      onEvent: (ev) => useStore.getState().applyEvent(ev),
      onState: (status, detail) => useStore.getState().setConn(status, detail),
    });
    onCleanup(() => sse.close());
  });
}
