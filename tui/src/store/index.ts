// Zustand root store. applyEvent is the single mutation point — every SSE
// frame routes through here. Snake-case kinds match EventDto from
// crates/openlet-protocol after axum's serde rename_all="snake_case".

import { createStore } from "zustand/vanilla";

import { upsertPartInMessage, updateMessageById, updatePartById } from "./reducers.js";
import { hydrateMessages } from "./message-hydration.js";

import type { FileBadge } from "../services/attachment-embedder.js";
import type { State, MessageView, PendingQuestion } from "./types.js";
export type { State, MessageView, PartView, OverlayEntry, OverlayKind, ConnSlice, PluginErrorView, PendingQuestion } from "./types.js";

// Sum two 4-decimal USD cost strings. A NaN parse falls back to the prior
// total so a malformed delta never zeroes the displayed cost.
function addCostStr(prev: string | undefined, delta: string): string {
  const a = Number.parseFloat(prev ?? "0");
  const b = Number.parseFloat(delta);
  const base = Number.isNaN(a) ? 0 : a;
  if (Number.isNaN(b)) return base.toFixed(4);
  return (base + b).toFixed(4);
}

export const useStore = createStore<State>((set) => ({
  conn: { status: "idle", attempt: 0, lastEventId: null },
  sessions: {},
  activeSessionId: null,
  messages: {},
  agents: [],
  plugins: [],
  pluginErrors: [],
  pendingPermissions: {},
  pendingQuestions: {},
  clientError: null,
  planMode: {},
  overlays: [],

  setConn: (status, detail) =>
    set((s) => ({
      conn: {
        status,
        attempt: detail?.attempt ?? s.conn.attempt,
        lastEventId: detail?.lastEventId ?? s.conn.lastEventId,
      },
    })),

  pushOverlay: (entry) => set((s) => ({ overlays: s.overlays.concat(entry) })),
  popOverlay: () => set((s) => ({ overlays: s.overlays.slice(0, -1) })),
  removeOverlay: (predicate) =>
    set((s) => ({ overlays: s.overlays.filter((e) => !predicate(e)) })),
  clearOverlays: () => set({ overlays: [] }),
  setAgents: (agents) => set({ agents }),
  setPlugins: (plugins) => set({ plugins }),
  setSessions: (sessions) =>
    set(() => ({
      sessions: Object.fromEntries(sessions.map((s) => [s.id, s])),
    })),
  setActiveSession: (id) => set({ activeSessionId: id }),
  setClientError: (message) => set({ clientError: message }),

  addUserMessage: (sessionId, messageId, text, badges: FileBadge[]) =>
    set((s) => {
      const list = s.messages[sessionId] ?? [];
      const userMsg: MessageView = {
        id: messageId,
        session_id: sessionId,
        role: "user",
        parts: [{ id: messageId, message_id: messageId, kind: "text", text, buffer: "", reasoning_buffer: "" }],
        created_at: new Date().toISOString(),
        badges: badges.length > 0 ? badges : undefined,
      };
      const idx = list.findIndex((m) => m.id === messageId);
      if (idx >= 0) {
        const next = list.slice();
        next[idx] = userMsg;
        return { messages: { ...s.messages, [sessionId]: next } };
      }
      return { messages: { ...s.messages, [sessionId]: list.concat(userMsg) } };
    }),

  hydrateSession: (sessionId, serverMessages) =>
    set((s) => {
      const existing = s.messages[sessionId] ?? [];
      const merged = hydrateMessages(existing, serverMessages);
      return { messages: { ...s.messages, [sessionId]: merged } };
    }),

  clearQuestion: (questionId) =>
    set((s) => {
      if (!s.pendingQuestions[questionId]) return {};
      const next = { ...s.pendingQuestions };
      delete next[questionId];
      return {
        pendingQuestions: next,
        overlays: s.overlays.filter(
          (e) => !(e.kind === "question" && e.questionId === questionId),
        ),
      };
    }),

  applyEvent: (ev) =>
    set((s) => {
      switch (ev.kind) {
        case "session_status": {
          const existing = s.sessions[ev.session_id];
          if (!existing) return {};
          return {
            sessions: {
              ...s.sessions,
              [ev.session_id]: { ...existing, status: ev.status, updated_at: ev.at },
            },
          };
        }

        case "message_created": {
          const list = s.messages[ev.session_id] ?? [];
          if (list.some((m) => m.id === ev.message_id)) return {};
          const emptyMsg: MessageView = {
            id: ev.message_id,
            session_id: ev.session_id,
            role: "assistant",
            parts: [],
            created_at: new Date().toISOString(),
          };
          return {
            messages: {
              ...s.messages,
              [ev.session_id]: list.concat(emptyMsg),
            },
          };
        }

        case "part_created": {
          const messages = upsertPartInMessage(
            s.messages,
            ev.session_id,
            ev.message_id,
            ev.part_id,
            (part) => part,
          );
          return { messages };
        }

        case "part_delta": {
          const messages = upsertPartInMessage(
            s.messages,
            ev.session_id,
            ev.message_id,
            ev.part_id,
            (part) => {
              if (ev.delta_kind === "text") return { ...part, buffer: part.buffer + ev.delta };
              if (ev.delta_kind === "reasoning")
                return { ...part, reasoning_buffer: part.reasoning_buffer + ev.delta };
              return part;
            },
          );
          return { messages };
        }

        case "part_updated": {
          const messages = updateMessageById(s.messages, ev.session_id, ev.message_id, (msg) =>
            updatePartById(msg, ev.part_id, (part) => ({
              ...part,
              text: (part.text ?? "") + part.buffer,
              buffer: "",
              status: "complete",
            })),
          );
          return messages ? { messages } : {};
        }

        case "step_finished": {
          const totalUsage = ev.usage
            ? ev.usage.input_tokens + ev.usage.output_tokens + ev.usage.reasoning_tokens
            : undefined;
          // Prompt tokens are the compaction anchor: `should_compact` compares
          // `usage.input_tokens` (the prompt size the model last measured)
          // against `context_window * compaction_threshold`. The context bar
          // uses the same number so it and the real trigger stay consistent.
          const contextTokens = ev.usage ? ev.usage.input_tokens : undefined;
          const messages = updateMessageById(s.messages, ev.session_id, ev.message_id, (msg) => ({
            ...msg,
            step_finish: {
              reason: ev.reason,
              usage_total: totalUsage,
              cost: ev.cost_decimal_str,
              context_tokens: contextTokens,
            },
          }));
          const session = s.sessions[ev.session_id];
          const sessions =
            session && ev.cost_decimal_str
              ? {
                  ...s.sessions,
                  [ev.session_id]: {
                    ...session,
                    cost_decimal_str: addCostStr(session.cost_decimal_str, ev.cost_decimal_str),
                  },
                }
              : undefined;
          if (messages && sessions) return { messages, sessions };
          if (messages) return { messages };
          if (sessions) return { sessions };
          return {};
        }

        case "permission_asked": {
          const already = s.overlays.some(
            (e) => e.kind === "permission" && e.askId === ev.request.ask_id,
          );
          return {
            pendingPermissions: { ...s.pendingPermissions, [ev.request.ask_id]: ev.request },
            overlays: already
              ? s.overlays
              : s.overlays.concat({ kind: "permission", askId: ev.request.ask_id }),
          };
        }

        case "permission_resolved": {
          const next = { ...s.pendingPermissions };
          delete next[ev.ask_id];
          return {
            pendingPermissions: next,
            overlays: s.overlays.filter(
              (e) => !(e.kind === "permission" && e.askId === ev.ask_id),
            ),
          };
        }

        case "question_requested": {
          const already = s.overlays.some(
            (e) => e.kind === "question" && e.questionId === ev.question_id,
          );
          const question: PendingQuestion = {
            session_id: ev.session_id,
            question_id: ev.question_id,
            header: ev.header,
            question: ev.question,
            options: ev.options,
            multi_select: ev.multi_select,
          };
          return {
            pendingQuestions: { ...s.pendingQuestions, [ev.question_id]: question },
            overlays: already
              ? s.overlays
              : s.overlays.concat({ kind: "question", questionId: ev.question_id }),
          };
        }

        case "plugin_error": {
          const errs = s.pluginErrors.concat({
            pluginId: ev.plugin_id,
            code: ev.code,
            message: ev.message,
            at: Date.now(),
          });
          return { pluginErrors: errs.slice(-20) };
        }

        case "plan_mode_entered": {
          return { planMode: { ...s.planMode, [ev.session_id]: true } };
        }

        case "plan_mode_exited": {
          const next = { ...s.planMode };
          delete next[ev.session_id];
          return { planMode: next };
        }

        case "error": {
          // Surface the server-side turn failure. Previously this frame was
          // dropped (return {}), so a turn that errored left the session in
          // "errored" with NO visible reason — the user just saw a dead
          // session. Route it to the persistent clientError banner (cleared on
          // the next successful submit) so the real code/message is shown.
          const detail = ev.message?.trim() ? ev.message : ev.code;
          return { clientError: `Agent error: ${detail}` };
        }

        case "heartbeat":
        default:
          return {};
      }
    }),
}));
