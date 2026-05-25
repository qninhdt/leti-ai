// Zustand root store. applyEvent is the single mutation point — every SSE
// frame routes through here. Snake-case kinds match EventDto from
// crates/openlet-protocol after axum's serde rename_all="snake_case".
// Phase plan dotted names (session.status etc.) are translated by sse.ts.

import { create } from "zustand";

import type { ConnState } from "../api/sse.js";
import type {
  AgentDto,
  EventDto,
  MessageDto,
  PartDto,
  PermissionRequestDto,
  PluginInfoDto,
  SessionDto,
} from "../api/types.js";

export type ViewKind =
  | { kind: "chat" }
  | { kind: "agent_picker" }
  | { kind: "session_picker" }
  | { kind: "permission"; askId: string }
  | { kind: "plugins" }
  | { kind: "help" };

export interface PartView extends PartDto {
  buffer: string;
  reasoning_buffer: string;
}

export interface MessageView extends MessageDto {
  parts: PartView[];
  step_finish?: { reason: string; usage_total?: number; cost?: string };
}

export interface ConnSlice {
  status: ConnState;
  attempt: number;
  lastEventId: number | null;
}

export interface PluginErrorView {
  pluginId: string;
  code: string;
  message: string;
  at: number;
}

export interface State {
  conn: ConnSlice;
  sessions: Record<string, SessionDto>;
  activeSessionId: string | null;
  messages: Record<string, MessageView[]>;
  agents: AgentDto[];
  plugins: PluginInfoDto[];
  pluginErrors: PluginErrorView[];
  pendingPermissions: Record<string, PermissionRequestDto>;
  /// Per-session plan-mode flag. Latched by `plan_mode_entered`,
  /// cleared by `plan_mode_exited`. The TUI reads this to render the
  /// banner and hint to the user that writes are blocked until exit.
  planMode: Record<string, boolean>;
  view: ViewKind;
  applyEvent: (ev: EventDto) => void;
  setConn: (status: ConnState, detail?: { attempt?: number; lastEventId?: number }) => void;
  setView: (view: ViewKind) => void;
  setAgents: (agents: AgentDto[]) => void;
  setPlugins: (plugins: PluginInfoDto[]) => void;
  setSessions: (sessions: SessionDto[]) => void;
  setActiveSession: (id: string | null) => void;
}

function emptyMessage(sessionId: string, messageId: string): MessageView {
  return {
    id: messageId,
    session_id: sessionId,
    role: "assistant",
    parts: [],
    created_at: new Date().toISOString(),
  };
}

function getOrCreateMessage(
  list: MessageView[],
  sessionId: string,
  messageId: string,
): { list: MessageView[]; index: number } {
  const idx = list.findIndex((m) => m.id === messageId);
  if (idx >= 0) return { list, index: idx };
  const next = list.concat(emptyMessage(sessionId, messageId));
  return { list: next, index: next.length - 1 };
}

function upsertPart(parts: PartView[], partId: string): { parts: PartView[]; index: number } {
  const idx = parts.findIndex((p) => p.id === partId);
  if (idx >= 0) return { parts, index: idx };
  const part: PartView = {
    id: partId,
    message_id: "",
    kind: "text",
    text: "",
    buffer: "",
    reasoning_buffer: "",
    status: "streaming",
  };
  return { parts: parts.concat(part), index: parts.length };
}

export const useStore = create<State>((set) => ({
  conn: { status: "idle", attempt: 0, lastEventId: null },
  sessions: {},
  activeSessionId: null,
  messages: {},
  agents: [],
  plugins: [],
  pluginErrors: [],
  pendingPermissions: {},
  planMode: {},
  view: { kind: "chat" },

  setConn: (status, detail) =>
    set((s) => ({
      conn: {
        status,
        attempt: detail?.attempt ?? s.conn.attempt,
        lastEventId: detail?.lastEventId ?? s.conn.lastEventId,
      },
    })),

  setView: (view) => set({ view }),
  setAgents: (agents) => set({ agents }),
  setPlugins: (plugins) => set({ plugins }),
  setSessions: (sessions) =>
    set(() => ({
      sessions: Object.fromEntries(sessions.map((s) => [s.id, s])),
    })),
  setActiveSession: (id) => set({ activeSessionId: id }),

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
          return {
            messages: {
              ...s.messages,
              [ev.session_id]: list.concat(emptyMessage(ev.session_id, ev.message_id)),
            },
          };
        }

        case "part_created": {
          const list = s.messages[ev.session_id] ?? [];
          const found = getOrCreateMessage(list, ev.session_id, ev.message_id);
          const msg = found.list[found.index]!;
          const upserted = upsertPart(msg.parts, ev.part_id);
          const nextMsg: MessageView = { ...msg, parts: upserted.parts };
          const nextList = found.list.slice();
          nextList[found.index] = nextMsg;
          return { messages: { ...s.messages, [ev.session_id]: nextList } };
        }

        case "part_delta": {
          const list = s.messages[ev.session_id] ?? [];
          const found = getOrCreateMessage(list, ev.session_id, ev.message_id);
          const msg = found.list[found.index]!;
          const upserted = upsertPart(msg.parts, ev.part_id);
          const part = upserted.parts[upserted.index]!;
          const next: PartView = { ...part };
          if (ev.delta_kind === "text") next.buffer = part.buffer + ev.delta;
          else if (ev.delta_kind === "reasoning") next.reasoning_buffer = part.reasoning_buffer + ev.delta;
          const nextParts = upserted.parts.slice();
          nextParts[upserted.index] = next;
          const nextMsg: MessageView = { ...msg, parts: nextParts };
          const nextList = found.list.slice();
          nextList[found.index] = nextMsg;
          return { messages: { ...s.messages, [ev.session_id]: nextList } };
        }

        case "part_updated": {
          const list = s.messages[ev.session_id] ?? [];
          const idx = list.findIndex((m) => m.id === ev.message_id);
          if (idx < 0) return {};
          const msg = list[idx]!;
          const partIdx = msg.parts.findIndex((p) => p.id === ev.part_id);
          if (partIdx < 0) return {};
          const part = msg.parts[partIdx]!;
          const next: PartView = {
            ...part,
            text: (part.text ?? "") + part.buffer,
            buffer: "",
            reasoning_buffer: "",
            status: "complete",
          };
          const nextParts = msg.parts.slice();
          nextParts[partIdx] = next;
          const nextMsg: MessageView = { ...msg, parts: nextParts };
          const nextList = list.slice();
          nextList[idx] = nextMsg;
          return { messages: { ...s.messages, [ev.session_id]: nextList } };
        }

        case "step_finished": {
          const list = s.messages[ev.session_id] ?? [];
          const idx = list.findIndex((m) => m.id === ev.message_id);
          if (idx < 0) return {};
          const msg = list[idx]!;
          const totalUsage = ev.usage
            ? ev.usage.input_tokens + ev.usage.output_tokens + ev.usage.reasoning_tokens
            : undefined;
          const nextMsg: MessageView = {
            ...msg,
            step_finish: { reason: ev.reason, usage_total: totalUsage, cost: ev.cost_decimal_str },
          };
          const nextList = list.slice();
          nextList[idx] = nextMsg;
          return { messages: { ...s.messages, [ev.session_id]: nextList } };
        }

        case "permission_asked": {
          return {
            pendingPermissions: { ...s.pendingPermissions, [ev.request.ask_id]: ev.request },
            view: { kind: "permission", askId: ev.request.ask_id },
          };
        }

        case "permission_resolved": {
          const next = { ...s.pendingPermissions };
          delete next[ev.ask_id];
          const view: ViewKind = s.view.kind === "permission" && s.view.askId === ev.ask_id
            ? { kind: "chat" }
            : s.view;
          return { pendingPermissions: next, view };
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

        case "error":
        case "heartbeat":
        default:
          return {};
      }
    }),
}));
