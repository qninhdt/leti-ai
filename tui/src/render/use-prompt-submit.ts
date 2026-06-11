// Builds the prompt submit pipeline shared by the editor. Ports the old Ink
// app.tsx `runSubmit`: a leading "/" routes to the slash-command registry
// (mapped onto store overlays/actions via CommandContext), otherwise the raw
// text is sent as a single text-part user message to the active session. The
// async work reads the zustand vanilla store directly (getState) so it needs
// no Solid reactivity, and surfaces failures through `setClientError` instead
// of leaving an unhandled rejection — the call site is a synchronous key
// handler, exactly as in the Ink build.

import { findCommand } from "../commands/registry.js";
import { useStore } from "../store/index.js";
import { randomId } from "../utils/id.js";

import type { AppRuntime } from "./app-context.js";
import type { CreateMessageDto } from "../api/types.js";

// Build a single text-part user message. The part id is client-generated and
// consumed by the server's part validation; it never leaves the request.
function textPrompt(text: string): CreateMessageDto {
  return {
    parts: [{ id: randomId(), message_id: "", kind: "text", text }],
  };
}

/// Returns `submit(text)` — the editor's single submission entry point. Empty
/// or whitespace-only buffers never reach the server (an empty Enter would
/// otherwise burn a turn); bare slash-commands like "/help" are still allowed.
export function createPromptSubmit(runtime: AppRuntime): (text: string) => Promise<void> {
  async function runSubmit(text: string): Promise<void> {
    const store = useStore.getState();

    if (text.startsWith("/")) {
      const [name] = text.slice(1).split(/\s+/);
      const cmd = findCommand(name ?? "");
      if (cmd) {
        await cmd.run({
          setView: (v) => store.setView(v as never),
          cancelTurn: async () => {
            if (store.activeSessionId) await runtime.client.abort(store.activeSessionId);
          },
          newSession: async () => {
            const agent = store.agents[0];
            if (!agent) return;
            const session = await runtime.client.createSession({ agent_id: agent.id });
            // Read fresh state after the await — the snapshot captured at the
            // top of runSubmit may be stale if an SSE frame inserted a session
            // while createSession was in flight; spreading the stale map would
            // clobber it.
            const fresh = useStore.getState();
            fresh.setSessions([...Object.values(fresh.sessions), session]);
            fresh.setActiveSession(session.id);
          },
          setMode: async (mode) => {
            if (store.activeSessionId) await runtime.client.setMode(store.activeSessionId, { mode });
          },
          enterPlanMode: async () => {
            // /plan only ENTERS plan mode; exit is the model's ExitPlanMode
            // tool. Submitting this as a synthetic user message keeps the
            // operator's intent auditable in the message log.
            if (!store.activeSessionId) return;
            await runtime.client.promptAsync(
              store.activeSessionId,
              textPrompt(
                "Please enter plan mode now using the enter_plan_mode tool, then gather context and produce a plan via exit_plan_mode.",
              ),
            );
          },
          exit: runtime.exit,
        });
        return;
      }
    }

    if (!store.activeSessionId) return;
    runtime.history.push(text);
    await runtime.client.promptAsync(store.activeSessionId, textPrompt(text));
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
