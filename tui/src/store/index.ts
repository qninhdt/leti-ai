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
  /// Last client-side error (failed prompt/command/session call). Surfaced
  /// as a banner so an async failure in the non-async input handler is
  /// visible to the user instead of becoming an unhandled rejection that
  /// could crash the process. Cleared on the next successful submit.
  clientError: string | null;
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
  setClientError: (message: string | null) => void;
}

// Sum two 4-decimal USD cost strings, returning a 4-decimal string. Used
// to accumulate per-turn step_finished costs into a session running total
// (the server's SessionDto carries no cost). A NaN parse falls back to the
// prior total so a malformed delta never zeroes the displayed cost.
function addCostStr(prev: string | undefined, delta: string): string {
  const a = Number.parseFloat(prev ?? "0");
  const b = Number.parseFloat(delta);
  const base = Number.isNaN(a) ? 0 : a;
  if (Number.isNaN(b)) return base.toFixed(4);
  return (base + b).toFixed(4);
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

// Immutably replace the message at `index` within a session's list and
// return the new top-level `messages` map. Every reducer branch that
// mutates a single message routes through here.
function withMessage(
  messages: Record<string, MessageView[]>,
  sessionId: string,
  list: MessageView[],
  index: number,
  message: MessageView,
): Record<string, MessageView[]> {
  const next = list.slice();
  next[index] = message;
  return { ...messages, [sessionId]: next };
}

// Ensure the message + part exist, then apply `update` to that part.
// Used by part_created (identity update) and part_delta (buffer append).
function upsertPartInMessage(
  messages: Record<string, MessageView[]>,
  sessionId: string,
  messageId: string,
  partId: string,
  update: (part: PartView) => PartView,
): Record<string, MessageView[]> {
  const list = messages[sessionId] ?? [];
  const { list: withMsg, index: msgIdx } = getOrCreateMessage(list, sessionId, messageId);
  const msg = withMsg[msgIdx]!;
  const { parts, index: partIdx } = upsertPart(msg.parts, partId);
  const nextParts = parts.slice();
  nextParts[partIdx] = update(parts[partIdx]!);
  return withMessage(messages, sessionId, withMsg, msgIdx, { ...msg, parts: nextParts });
}

// Apply `update` to an existing message looked up by id. Returns null
// (no-op) when the message — or, via the updater, a referenced part —
// cannot be found. Used by part_updated and step_finished.
function updateMessageById(
  messages: Record<string, MessageView[]>,
  sessionId: string,
  messageId: string,
  update: (msg: MessageView) => MessageView | null,
): Record<string, MessageView[]> | null {
  const list = messages[sessionId] ?? [];
  const idx = list.findIndex((m) => m.id === messageId);
  if (idx < 0) return null;
  const updated = update(list[idx]!);
  return updated === null ? null : withMessage(messages, sessionId, list, idx, updated);
}

// Immutably replace a part within a message, looked up by id. Returns
// null when the part is absent so callers can treat it as a no-op.
function updatePartById(
  msg: MessageView,
  partId: string,
  update: (part: PartView) => PartView,
): MessageView | null {
  const idx = msg.parts.findIndex((p) => p.id === partId);
  if (idx < 0) return null;
  const parts = msg.parts.slice();
  parts[idx] = update(parts[idx]!);
  return { ...msg, parts };
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
  clientError: null,
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
  setClientError: (message) => set({ clientError: message }),

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
          // Create-only: an existing part is left untouched (identity update).
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
              reasoning_buffer: "",
              status: "complete",
            })),
          );
          return messages ? { messages } : {};
        }

        case "step_finished": {
          const totalUsage = ev.usage
            ? ev.usage.input_tokens + ev.usage.output_tokens + ev.usage.reasoning_tokens
            : undefined;
          const messages = updateMessageById(s.messages, ev.session_id, ev.message_id, (msg) => ({
            ...msg,
            step_finish: { reason: ev.reason, usage_total: totalUsage, cost: ev.cost_decimal_str },
          }));
          // Accumulate the per-turn cost into the session's running total.
          // The server's SessionDto carries no cost; the live figure lives
          // only on the step_finished event, so the status bar gets its
          // value from here.
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
