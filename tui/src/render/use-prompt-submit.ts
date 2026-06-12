// Builds the prompt submit pipeline shared by the editor. A leading "/"
// routes to the slash-command registry (mapped onto store overlays/actions via
// CommandContext), otherwise the raw text is sent as a single text-part user
// message to the active session. The async work reads the zustand vanilla store
// directly (getState) so it needs no Solid reactivity, and surfaces failures
// through `setClientError` instead of leaving an unhandled rejection.

import { findCommand } from "../commands/registry.js";
import { useStore } from "../store/index.js";
import { randomId } from "../utils/id.js";
import { embedMentions } from "../services/attachment-embedder.js";
import { createCommandContext } from "./command-context.js";

import type { AppRuntime } from "./app-context.js";
import type { CreateMessageDto } from "../api/types.js";

// Build a single text-part user message.
function textPrompt(text: string): CreateMessageDto {
  return {
    parts: [{ id: randomId(), message_id: "", kind: "text", text }],
  };
}

/// Returns `submit(text)` — the editor's single submission entry point.
export function createPromptSubmit(runtime: AppRuntime): (text: string) => Promise<void> {
  const ctx = createCommandContext(runtime);

  async function runSubmit(text: string): Promise<void> {
    if (text.startsWith("/")) {
      const [name] = text.slice(1).split(/\s+/);
      const cmd = findCommand(name ?? "");
      if (cmd) {
        await cmd.run(ctx);
        return;
      }
    }

    const store = useStore.getState();
    if (!store.activeSessionId) return;
    const sessionId = store.activeSessionId;

    // Resolve @-mentions: embed file content into the outgoing prompt, collect
    // badge descriptors for the optimistic message.
    const { promptSection, badges } = await embedMentions(text, runtime.client);
    runtime.history.push(text);
    const ack = await runtime.client.promptAsync(sessionId, textPrompt(text + promptSection));

    // Add the optimistic user message keyed by the server-assigned id.
    useStore.getState().addUserMessage(sessionId, ack.message_id, text, badges);

    // Surface any per-file resolution failure as a banner.
    const failed = badges.filter((b) => b.error);
    if (failed.length > 0) {
      useStore
        .getState()
        .setClientError(`could not attach: ${failed.map((b) => b.path).join(", ")}`);
    }
  }

  return async function submit(text: string): Promise<void> {
    if (!text.trim()) return;
    const store = useStore.getState();
    try {
      store.setClientError(null);
      await runSubmit(text);
    } catch (err) {
      store.setClientError(err instanceof Error ? err.message : String(err));
    }
  };
}
