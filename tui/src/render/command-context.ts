// Builds the slash-command execution context shared by the prompt submit path
// and the command palette. Centralizes the wiring from a Command's abstract
// actions (setView, newSession, setMode, …) onto the concrete store + REST
// client, so both entry points run commands identically. Reads the zustand
// vanilla store via getState (no Solid reactivity needed) and reads fresh after
// each await so a concurrently-inserted session is not clobbered.

import { useStore } from "../store/index.js";
import { randomId } from "../utils/id.js";

import type { AppRuntime } from "./app-context.js";
import type { CommandContext } from "../commands/registry.js";
import type { CreateMessageDto } from "../api/types.js";

function textPrompt(text: string): CreateMessageDto {
  return { parts: [{ id: randomId(), message_id: "", kind: "text", text }] };
}

export function createCommandContext(runtime: AppRuntime): CommandContext {
  return {
    setView: (v) => useStore.getState().setView(v as never),
    cancelTurn: async () => {
      const id = useStore.getState().activeSessionId;
      if (id) await runtime.client.abort(id);
    },
    newSession: async () => {
      const agent = useStore.getState().agents[0];
      if (!agent) return;
      const session = await runtime.client.createSession({ agent_id: agent.id });
      const fresh = useStore.getState();
      fresh.setSessions([...Object.values(fresh.sessions), session]);
      fresh.setActiveSession(session.id);
    },
    setMode: async (mode) => {
      const id = useStore.getState().activeSessionId;
      if (id) await runtime.client.setMode(id, { mode });
    },
    enterPlanMode: async () => {
      // /plan only ENTERS plan mode; exit is the model's ExitPlanMode tool.
      // Submitting this as a synthetic user message keeps the operator's intent
      // auditable in the message log.
      const id = useStore.getState().activeSessionId;
      if (!id) return;
      await runtime.client.promptAsync(
        id,
        textPrompt(
          "Please enter plan mode now using the enter_plan_mode tool, then gather context and produce a plan via exit_plan_mode.",
        ),
      );
    },
    exit: runtime.exit,
  };
}
