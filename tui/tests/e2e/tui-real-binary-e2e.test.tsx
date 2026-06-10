// Zero-mock real-binary E2E tier. Renders the real <App> against an actual
// compiled `openlet-server` process talking to the in-process Rust
// `mock-openai-service` — real axum HTTP, real SSE socket, real provider
// stream. This is the FE half of the staging "zero-mock" mandate; the Node
// wire-double tier (`tui-live-e2e.test.tsx`) stays the fast default lane.
//
// GATED: the whole suite is skipped unless OPENLET_TUI_REAL_E2E=1, so a
// plain `npm test` on a box without a Rust toolchain stays green. Run with:
//   OPENLET_TUI_REAL_E2E=1 npm run test:e2e:real
//
// The real-OpenRouter sub-tier is double-gated (also needs OPENLET_LIVE_E2E=1
// + OPENROUTER_API_KEY) and asserts shape only, never exact model words.

import { render } from "ink-testing-library";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { App } from "../../src/app.js";
import { createClient } from "../../src/api/client.js";
import { useStore } from "../../src/store/index.js";
import {
  openrouterE2eEnabled,
  realE2eEnabled,
  spawnRealServerWithMock,
  spawnRealServerWithOpenRouter,
  type RealServer,
} from "./spawn-real-server.js";

import type { PromptHistory } from "../../src/hooks/use-prompt-history.js";

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

// SSE + a real LLM turn are slower than the Node double; poll longer.
async function waitFor(pred: () => boolean, timeoutMs = 20_000): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    if (pred()) return;
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error("waitFor timed out");
}

// ── Tier B: real binary + deterministic in-process mock LLM ─────────────
describe.skipIf(!realE2eEnabled())("TUI real-binary E2E (mock LLM)", () => {
  let server: RealServer;
  let baseUrl: string;

  beforeEach(async () => {
    resetStore();
    server = await spawnRealServerWithMock();
    baseUrl = server.baseUrl;
  }, 60_000);

  afterEach(() => {
    server?.kill();
  });

  it("bootstraps agents + plugins from the real server", async () => {
    const client = createClient({ baseUrl });
    const { unmount } = render(
      <App client={client} baseUrl={baseUrl} history={memoryHistory()} />,
    );

    await waitFor(() => useStore.getState().agents.length > 0);
    // Real AgentDto carries display_name/workspace_root (not the Node
    // double's `name`), so assert presence, not a literal name.
    expect(useStore.getState().agents.length).toBeGreaterThan(0);
    // core-tools registers the eight built-ins — matches the Rust
    // live_e2e_plugin_agent.rs assertion.
    await waitFor(() => useStore.getState().plugins.length > 0);
    expect(
      useStore.getState().plugins.some((p) => p.id.includes("core-tools")),
    ).toBe(true);

    unmount();
  }, 30_000);

  it("streams a simple_text turn over real SSE and renders the text", async () => {
    const client = createClient({ baseUrl });
    const session = await client.createSession({ agent_id: undefined as never });
    useStore.getState().setSessions([session]);
    useStore.getState().setActiveSession(session.id);

    const { lastFrame, unmount } = render(
      <App client={client} baseUrl={baseUrl} history={memoryHistory()} />,
    );
    await waitFor(() => useStore.getState().conn.status === "open");

    // The mock's simple_text scenario streams "Hello" + ", world".
    await client.promptAsync(session.id, {
      parts: [{ kind: "text", id: crypto.randomUUID(), message_id: crypto.randomUUID(), text: "PARITY_SCENARIO:simple_text hi" }],
    });

    await waitFor(() => {
      const msgs = useStore.getState().messages[session.id] ?? [];
      return msgs.some((m) => m.parts.some((p) => (p.text ?? "").includes("Hello, world")));
    });
    await waitFor(() => (lastFrame() ?? "").includes("Hello, world"));
    expect(lastFrame()).toContain("Hello, world");

    unmount();
  }, 30_000);

  it("submitting a prompt round-trips a streamed turn through the real server", async () => {
    const client = createClient({ baseUrl });
    const session = await client.createSession({ agent_id: undefined as never });
    useStore.getState().setSessions([session]);
    useStore.getState().setActiveSession(session.id);

    const { stdin, lastFrame, unmount } = render(
      <App client={client} baseUrl={baseUrl} history={memoryHistory()} />,
    );
    await waitFor(() => useStore.getState().conn.status === "open");

    // Same stale-closure retry the Node-double test documents: ink
    // re-registers PromptEditor's useInput in a passive effect, so an Enter
    // fired too early can hit a stale handler. Retry type+Enter until the
    // turn actually streams back.
    const typed = "PARITY_SCENARIO:simple_text go";
    const streamed = () => {
      const msgs = useStore.getState().messages[session.id] ?? [];
      return msgs.some((m) =>
        m.parts.some((p) => (p.text ?? "").includes("Hello, world")),
      );
    };
    for (let attempt = 0; attempt < 30 && !streamed(); attempt++) {
      if (!(lastFrame() ?? "").includes(typed)) {
        stdin.write(typed);
        await waitFor(() => (lastFrame() ?? "").includes(typed), 5_000);
        await new Promise((r) => setTimeout(r, 30));
      }
      stdin.write("\r");
      await new Promise((r) => setTimeout(r, 100));
    }

    await waitFor(streamed);
    expect(streamed()).toBe(true);

    unmount();
  }, 30_000);

  it("a real with_tool_call turn flips the view to the permission modal", async () => {
    const client = createClient({ baseUrl });
    const session = await client.createSession({ agent_id: undefined as never });
    useStore.getState().setSessions([session]);
    useStore.getState().setActiveSession(session.id);

    const { unmount } = render(
      <App client={client} baseUrl={baseUrl} history={memoryHistory()} />,
    );
    await waitFor(() => useStore.getState().conn.status === "open");

    // Default session mode is WorkspaceWrite → bash falls through to Ask →
    // Decision::Pending → the real dispatcher publishes permission.asked.
    // This is the FE half of the Phase-1 dispatcher fix proven end to end.
    await client.promptAsync(session.id, {
      parts: [{ kind: "text", id: crypto.randomUUID(), message_id: crypto.randomUUID(), text: "PARITY_SCENARIO:with_tool_call run" }],
    });

    await waitFor(() => useStore.getState().view.kind === "permission");
    expect(useStore.getState().view.kind).toBe("permission");
    expect(Object.keys(useStore.getState().pendingPermissions).length).toBeGreaterThan(0);

    unmount();
  }, 30_000);
});

// ── Real-OpenRouter sub-tier: double-gated, shape-only ──────────────────
describe.skipIf(!openrouterE2eEnabled())("TUI real-binary E2E (real OpenRouter)", () => {
  let server: RealServer;
  let baseUrl: string;

  beforeEach(async () => {
    resetStore();
    server = await spawnRealServerWithOpenRouter();
    baseUrl = server.baseUrl;
  }, 60_000);

  afterEach(() => {
    server?.kill();
  });

  it("streams a real completion into the rendered frame", async () => {
    const client = createClient({ baseUrl });
    const session = await client.createSession({ agent_id: undefined as never });
    useStore.getState().setSessions([session]);
    useStore.getState().setActiveSession(session.id);

    const { unmount } = render(
      <App client={client} baseUrl={baseUrl} history={memoryHistory()} />,
    );
    await waitFor(() => useStore.getState().conn.status === "open");

    await client.promptAsync(session.id, {
      parts: [{ kind: "text", id: crypto.randomUUID(), message_id: crypto.randomUUID(), text: "Reply with exactly one word: ok" }],
    });

    // Shape only — assert non-empty streamed assistant text, never the
    // exact words (model output is non-deterministic).
    await waitFor(() => {
      const msgs = useStore.getState().messages[session.id] ?? [];
      return msgs.some((m) => m.parts.some((p) => (p.text ?? "").trim().length > 0));
    }, 45_000);

    const msgs = useStore.getState().messages[session.id] ?? [];
    const hasText = msgs.some((m) => m.parts.some((p) => (p.text ?? "").trim().length > 0));
    expect(hasText).toBe(true);

    unmount();
  }, 60_000);
});
