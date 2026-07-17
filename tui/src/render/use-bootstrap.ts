// Boots the app: parallel-loads agents/plugins/sessions into the store, then
// opens one process-lifetime SSE stream and routes frames to applyEvent.
// Route switches only hydrate the selected transcript; they never interrupt
// parent/child lifecycle updates.

import { createEffect, onCleanup } from "solid-js";

import { connectSse } from "../api/sse.js";
import { useStore } from "../store/index.js";
import { useStoreSelector } from "./store-bridge.js";
import { useRuntime } from "./app-context.js";
import { createHydrationController } from "./hydration-controller.js";
import { createEventPump } from "./event-pump.js";
import { warmTreeSitter } from "./warm-tree-sitter.js";

import type { LetiClient } from "../api/client.js";

async function loadInitial(client: LetiClient): Promise<void> {
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

  // Spawn the highlight worker + load parser WASM now, off the streaming path,
  // so the first assistant tokens don't trigger a cold tree-sitter start (which
  // makes the streaming block flash unstyled<->styled every token).
  warmTreeSitter();

  const hydration = createHydrationController(runtime.client);

  const activeSessionId = useStoreSelector((s) => s.activeSessionId);
  // Route switches hydrate the selected transcript but never replace the
  // process-lifetime SSE connection: parent and child activity continues to
  // update while either route is visible.
  createEffect(() => {
    const sessionId = activeSessionId();
    // Fetch server-authoritative bodies for the newly active session so its
    // tool calls (name/args/results) render immediately — the SSE stream alone
    // carries only part ids.
    if (sessionId) hydration.refresh(sessionId);

  });

  createEffect(() => {
    // Coalesce the per-token part_delta flood to ~30fps before it hits the
    // store; every other event flushes the buffer first, then applies. The
    // pump is per-connection so its timer is torn down with the stream.
    // hydration.onEvent stays on the raw event (it only reacts to
    // session_status/message_created, never deltas) so hydration timing is
    // unchanged.
    const pump = createEventPump((ev) => useStore.getState().applyEvent(ev));

    const sse = connectSse({
      baseUrl: runtime.baseUrl,
      token: runtime.token,
      onEvent: (ev) => {
        pump.push(ev);
        hydration.onEvent(ev);
        if (ev.kind === "subagent_spawned") {
          hydration.refresh(ev.child_session_id);
        }
      },
      onState: (status, detail) => useStore.getState().setConn(status, detail),
    });
    onCleanup(() => {
      sse.close();
      pump.dispose();
    });
  });
}
