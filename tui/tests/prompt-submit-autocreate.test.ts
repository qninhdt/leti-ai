// Regression: submitting a prompt on the home screen (no active session) must
// lazily create + activate a session and send the prompt, instead of silently
// dropping it. Guards the bug where Enter cleared the textarea and nothing was
// sent because runSubmit early-returned on a null activeSessionId.

import { afterEach, describe, expect, it } from "vitest";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { createPromptSubmit } from "../src/render/use-prompt-submit.js";
import { useStore } from "../src/store/index.js";
import { PromptHistory } from "../src/services/prompt-history.js";

import type { AppRuntime } from "../src/render/app-context.js";
import type { OpenletClient } from "../src/api/client.js";
import type { AgentDto, SessionDto } from "../src/api/types.js";

const AGENT: AgentDto = { id: "agent-1", display_name: "Default" };

function session(id: string): SessionDto {
  return {
    id,
    agent_id: AGENT.id,
    status: "idle",
    permission_mode: "read_only",
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    cost_decimal_str: "0.0000",
  };
}

// Records createSession / promptAsync calls; no @-mentions so getFileContent is
// never hit.
function fakeClient(created: SessionDto) {
  const calls = { createSession: 0, prompt: [] as Array<{ sessionId: string }> };
  const client = {
    createSession: async () => {
      calls.createSession++;
      return created;
    },
    promptAsync: async (sessionId: string) => {
      calls.prompt.push({ sessionId });
      return { message_id: "msg-1" };
    },
  } as unknown as OpenletClient;
  return { client, calls };
}

function runtime(client: OpenletClient): AppRuntime {
  const historyFile = join(mkdtempSync(join(tmpdir(), "openlet-history-")), "history.jsonl");
  return { client, baseUrl: "http://x", history: new PromptHistory(historyFile), exit: () => {} };
}

// Reset the shared vanilla store between cases.
afterEach(() => {
  useStore.setState({ agents: [], sessions: {}, activeSessionId: null, clientError: null });
});

describe("prompt submit — lazy session creation", () => {
  it("creates + activates a session and sends the prompt when none is active", async () => {
    useStore.setState({ agents: [AGENT], sessions: {}, activeSessionId: null });
    const { client, calls } = fakeClient(session("sess-new"));

    await createPromptSubmit(runtime(client))("hello world");

    expect(calls.createSession).toBe(1);
    expect(calls.prompt).toEqual([{ sessionId: "sess-new" }]);
    expect(useStore.getState().activeSessionId).toBe("sess-new");
    expect(useStore.getState().clientError).toBeNull();
  });

  it("reuses the active session without creating a new one", async () => {
    useStore.setState({
      agents: [AGENT],
      sessions: { "sess-existing": session("sess-existing") },
      activeSessionId: "sess-existing",
    });
    const { client, calls } = fakeClient(session("sess-unused"));

    await createPromptSubmit(runtime(client))("follow up");

    expect(calls.createSession).toBe(0);
    expect(calls.prompt).toEqual([{ sessionId: "sess-existing" }]);
  });

  it("surfaces a client error (not a silent drop) when no agent exists", async () => {
    useStore.setState({ agents: [], sessions: {}, activeSessionId: null });
    const { client, calls } = fakeClient(session("sess-none"));

    await createPromptSubmit(runtime(client))("hello");

    expect(calls.createSession).toBe(0);
    expect(calls.prompt).toEqual([]);
    expect(useStore.getState().clientError).toBe("no agent available to start a session");
  });
});
