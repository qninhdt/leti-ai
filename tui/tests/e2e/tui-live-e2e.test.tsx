// Node wire-double tier (fast FE pipeline). Renders the real <App> via
// ink-testing-library against a live Node http server speaking the openlet
// wire contract — real fetch, real EventSource, real store + render — but NOT
// the Rust binary. This is the fast, self-contained default `npm test` lane
// (no cargo coupling). The zero-mock real-binary tier lives in
// `tui-real-binary-e2e.test.tsx` (gated by OPENLET_TUI_REAL_E2E=1).
//
// Exercises the full FE pipeline the way a human operator would: bootstrap →
// create session → submit a prompt over real HTTP → receive a streamed
// assistant turn over a real SSE socket → assert the rendered frame.
//
// The store is a module-level zustand singleton, so each test resets it to a
// clean slate before rendering.

import { render } from "ink-testing-library";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { App } from "../../src/app.js";
import { createClient } from "../../src/api/client.js";
import { useStore } from "../../src/store/index.js";
import { LiveTestServer } from "./live-test-server.js";

import type { PromptHistory } from "../../src/hooks/use-prompt-history.js";

// In-memory PromptHistory double — no filesystem touch in tests.
function memoryHistory(): PromptHistory {
  const entries: string[] = [];
  return {
    push: (text: string) => void entries.push(text),
    list: () => entries.slice(),
  } as unknown as PromptHistory;
}

const RESET = {
  conn: { status: "idle" as const, attempt: 0, lastEventId: null },
  sessions: {},
  activeSessionId: null,
  messages: {},
  agents: [],
  plugins: [],
  pluginErrors: [],
  pendingPermissions: {},
  clientError: null,
  planMode: {},
  view: { kind: "chat" as const },
};

function resetStore(): void {
  useStore.setState(RESET, false);
}

// Poll a predicate until true or timeout — SSE + React render are async.
async function waitFor(pred: () => boolean, timeoutMs = 4000): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    if (pred()) return;
    await new Promise((r) => setTimeout(r, 25));
  }
  throw new Error("waitFor timed out");
}

describe("TUI live E2E", () => {
  let server: LiveTestServer;
  let baseUrl: string;

  beforeEach(async () => {
    resetStore();
    server = new LiveTestServer();
    baseUrl = await server.start();
  });

  afterEach(async () => {
    await server.stop();
  });

  it("bootstraps agents + plugins from the live server", async () => {
    const client = createClient({ baseUrl });
    const { unmount } = render(
      <App client={client} baseUrl={baseUrl} history={memoryHistory()} />,
    );

    await waitFor(() => useStore.getState().agents.length > 0);
    expect(useStore.getState().agents[0]!.display_name).toBe("general");
    expect(useStore.getState().plugins[0]!.id).toBe("core-tools");

    unmount();
  });

  it("streams an assistant turn over real SSE and renders the text", async () => {
    const client = createClient({ baseUrl });

    // Pre-seed an active session so the SSE subscription targets it and
    // submit() has somewhere to send the prompt.
    const session = await client.createSession({ agent_id: server.agentId() });
    useStore.getState().setSessions([session]);
    useStore.getState().setActiveSession(session.id);

    const { lastFrame, unmount } = render(
      <App client={client} baseUrl={baseUrl} history={memoryHistory()} />,
    );

    // Wait for the SSE channel to be open before pushing frames.
    await waitFor(() => server.clientCount() > 0);

    // Server streams a full assistant turn.
    server.pushAssistantTurn(session.id, "Hello from the agent", 3);

    // The store assembles deltas into a completed part; the rendered
    // frame shows the text.
    await waitFor(() => {
      const msgs = useStore.getState().messages[session.id] ?? [];
      const part = msgs[0]?.parts[0];
      return part?.text === "Hello from the agent";
    });
    await waitFor(() => (lastFrame() ?? "").includes("Hello from the agent"));

    expect(lastFrame()).toContain("Hello from the agent");
    unmount();
  });

  it("submitting a prompt POSTs prompt_async to the live server", async () => {
    const client = createClient({ baseUrl });
    const session = await client.createSession({ agent_id: server.agentId() });
    useStore.getState().setSessions([session]);
    useStore.getState().setActiveSession(session.id);

    const { stdin, lastFrame, unmount } = render(
      <App client={client} baseUrl={baseUrl} history={memoryHistory()} />,
    );
    await waitFor(() => server.clientCount() > 0);

    // Drive the keyboard the way a human would. ink re-registers the
    // PromptEditor's `useInput` handler in a passive effect that flushes
    // AFTER the frame is committed, so an Enter fired immediately after the
    // text renders can hit a stale closure (which would also wipe the
    // buffer via its captured empty `value`). Retry the type+Enter until
    // the server actually receives the prompt — deterministic, no fixed
    // sleeps, and self-correcting if a keystroke lands on a stale handler.
    for (let attempt = 0; attempt < 20 && server.lastPrompt === null; attempt++) {
      if (!(lastFrame() ?? "").includes("ship it")) {
        stdin.write("ship it");
        await waitFor(() => (lastFrame() ?? "").includes("ship it"));
        // Yield a macrotask so ink's useInput effect re-registers with the
        // current buffer before we press Enter.
        await new Promise((r) => setTimeout(r, 30));
      }
      stdin.write("\r");
      await new Promise((r) => setTimeout(r, 50));
    }

    await waitFor(() => server.lastPrompt !== null);
    expect(server.lastPrompt!.sessionId).toBe(session.id);
    const parts = (server.lastPrompt!.body as { parts: { text: string }[] }).parts;
    expect(parts[0]!.text).toBe("ship it");

    unmount();
  });

  it("a permission_asked SSE frame flips the view to the permission modal", async () => {
    const client = createClient({ baseUrl });
    const session = await client.createSession({ agent_id: server.agentId() });
    useStore.getState().setSessions([session]);
    useStore.getState().setActiveSession(session.id);

    const { lastFrame, unmount } = render(
      <App client={client} baseUrl={baseUrl} history={memoryHistory()} />,
    );
    await waitFor(() => server.clientCount() > 0);

    server.push({
      event: "permission.asked",
      id: 1,
      data: {
        session_id: session.id,
        request: {
          ask_id: "ask-e2e-1",
          session_id: session.id,
          permission: "bash:rm",
          tool_name: "bash",
        },
      },
    });

    await waitFor(() => useStore.getState().view.kind === "permission");
    expect(useStore.getState().pendingPermissions["ask-e2e-1"]).toBeDefined();
    // The modal renders the tool name.
    await waitFor(() => (lastFrame() ?? "").length > 0);

    unmount();
  });
});
