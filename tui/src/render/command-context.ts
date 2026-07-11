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

// The server keeps the most recent PRESERVE_RECENT (4) messages verbatim and
// only compacts when the stored count EXCEEDS that floor, so a conversation
// needs more than 4 messages — i.e. at least 5 — before /compact folds
// anything. Mirrored here purely to phrase a helpful "send N more" notice; the
// server remains the authority (its ack decides what actually happens).
const COMPACT_MIN_MESSAGES = 5;

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
    compact: async () => {
      const store = useStore.getState();
      const id = store.activeSessionId;
      if (!id) {
        store.setNotice("no active session to compact");
        return;
      }
      // Compaction is a background turn: the ack reports only whether one was
      // dispatched. `compacted:false` means the conversation is at/under the
      // preserved-recent floor (nothing older than the kept-verbatim tail to
      // fold), so the server no-ops. That is the common surprise — the notice
      // must say WHY nothing happened and how many more messages are needed,
      // not a dead-end "nothing to compact". On success the divider appears
      // once the async summarization turn settles + re-hydrates.
      const ack = await runtime.client.compact(id);
      if (ack.compacted) {
        store.setNotice("compacting conversation…");
        return;
      }
      // Mirror the server's floor: it keeps the most recent PRESERVE_RECENT
      // messages verbatim and only compacts when the count exceeds that, so a
      // short conversation has nothing to fold yet.
      const count = store.messages[id]?.length ?? 0;
      const needed = COMPACT_MIN_MESSAGES - count;
      store.setNotice(
        needed > 0
          ? `too short to compact — send ${needed} more message${needed === 1 ? "" : "s"} first`
          : "nothing to compact yet",
      );
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
