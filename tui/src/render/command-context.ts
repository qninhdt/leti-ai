// Builds the slash-command execution context shared by the prompt submit path
// and the command palette. Centralizes the wiring from a Command's abstract
// actions (newSession, setMode, ...) onto the concrete store + REST client, so
// both entry points run commands identically. Reads the zustand vanilla store
// via getState (no Solid reactivity needed) and reads fresh after each await so
// a concurrently-inserted session is not clobbered.

import { useStore } from "../store/index.js";
import { randomId } from "../utils/id.js";
import { createAndActivateSession } from "../services/session-actions.js";

import type { AppRuntime } from "./app-context.js";
import type { CommandContext } from "../commands/registry.js";
import type { CreateMessageDto } from "../api/types.js";
import type { OverlayEntry } from "../store/index.js";

function textPrompt(text: string): CreateMessageDto {
  return { parts: [{ id: randomId(), message_id: "", kind: "text", text }] };
}

// Map the abstract view kind from slash commands onto an overlay entry.
function viewKindToOverlay(view: { kind: string; askId?: string }): OverlayEntry | null {
  switch (view.kind) {
    case "agent_picker":
      return { kind: "agent_picker" };
    case "session_picker":
      return { kind: "session_picker" };
    case "help":
      return { kind: "help" };
    case "plugins":
      return { kind: "plugins" };
    case "permission":
      return view.askId ? { kind: "permission", askId: view.askId } : null;
    default:
      return null;
  }
}

export function createCommandContext(runtime: AppRuntime): CommandContext {
  return {
    setView: (v) => {
      const overlay = viewKindToOverlay(v);
      if (overlay) useStore.getState().pushOverlay(overlay);
    },
    cancelTurn: async () => {
      const id = useStore.getState().activeSessionId;
      if (id) await runtime.client.abort(id);
    },
    newSession: async () => {
      await createAndActivateSession(runtime.client);
    },
    setMode: async (mode) => {
      const id = useStore.getState().activeSessionId;
      if (id) await runtime.client.setMode(id, { mode });
    },
    enterPlanMode: async () => {
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
