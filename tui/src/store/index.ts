// Zustand root store. applyEvent is the single mutation point — every SSE
// frame routes through here. Snake-case kinds match EventDto from
// crates/openlet-protocol after axum's serde rename_all="snake_case".

import { createStore } from "zustand/vanilla";

import { hydrateMessages } from "./message-hydration.js";
import { applyEvent } from "./apply-event.js";

import type { FileBadge } from "../services/attachment-embedder.js";
import type { State, MessageView } from "./types.js";
export type { State, MessageView, PartView, OverlayEntry, OverlayKind, ConnSlice, PluginErrorView, PendingQuestion, SubagentView } from "./types.js";

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
  notice: null,
  planMode: {},
  overlays: [],
  subagents: {},
  roster: {},
  idleNotices: [],

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
  setNotice: (text) => set((s) => ({ notice: { text, seq: (s.notice?.seq ?? 0) + 1 } })),

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

  applyEvent: (ev) => set((s) => applyEvent(s, ev)),
}));
