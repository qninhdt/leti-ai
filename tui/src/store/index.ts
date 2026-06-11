// Zustand root store. applyEvent is the single mutation point — every SSE
// frame routes through here. Snake-case kinds match EventDto from
// crates/openlet-protocol after axum's serde rename_all="snake_case".
// Phase plan dotted names (session.status etc.) are translated by sse.ts.

import { create } from "zustand";

import type { ConnState } from "../api/sse.js";
import type { FileBadge } from "../services/attachment-embedder.js";
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

// Overlay stack entries. Each carries the payload its dialog needs to resolve
// itself — notably the permission entry MUST carry `askId` so a reply targets
// the exact request, even with multiple permissions pending (pendingPermissions
// is keyed by askId). A bare kind would make ≥2 concurrent asks ambiguous.
export type OverlayEntry =
  | { kind: "permission"; askId: string }
  | { kind: "agent_picker" }
  | { kind: "session_picker" }
  | { kind: "help" }
  | { kind: "plugins" }
  | { kind: "command_palette" };

export type OverlayKind = OverlayEntry["kind"];

// Transition shim: map the legacy modal ViewKinds (still emitted by the slash
// command registry via CommandContext.setView) onto overlay entries. Removed
// once commands push overlays directly (Phase 5).
function viewToOverlay(view: ViewKind): OverlayEntry | null {
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
      return { kind: "permission", askId: view.askId };
    default:
      return null;
  }
}

export interface PartView extends PartDto {
  buffer: string;
  reasoning_buffer: string;
}

export interface MessageView extends MessageDto {
  parts: PartView[];
  step_finish?: { reason: string; usage_total?: number; cost?: string };
  /// File-mention badge chips for a user message (Phase 6 @-mentions). Carried
  /// on the optimistic user message and preserved when the SSE echo arrives.
  badges?: FileBadge[];
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
  /// Overlay stack rendered atop the active route (dialogs, pickers, command
  /// palette). Top of stack = last element. Replaces the view-swap model for
  /// modal surfaces; `view` is retained only for the chat/route distinction
  /// during the Phase 5 transition.
  overlays: OverlayEntry[];
  applyEvent: (ev: EventDto) => void;
  setConn: (status: ConnState, detail?: { attempt?: number; lastEventId?: number }) => void;
  setView: (view: ViewKind) => void;
  pushOverlay: (entry: OverlayEntry) => void;
  popOverlay: () => void;
  removeOverlay: (predicate: (entry: OverlayEntry) => boolean) => void;
  clearOverlays: () => void;
  setAgents: (agents: AgentDto[]) => void;
  setPlugins: (plugins: PluginInfoDto[]) => void;
  setSessions: (sessions: SessionDto[]) => void;
  setActiveSession: (id: string | null) => void;
  setClientError: (message: string | null) => void;
  /// Append an optimistic user message (role:"user") carrying the raw text and
  /// file-mention badges. The SSE stream never produces a user message
  /// (message_created hardcodes role:"assistant"), so the TUI must add it
  /// itself. The client-generated id means a later server echo is deduped by
  /// the message_created handler — preserving the FE badges.
  addUserMessage: (sessionId: string, messageId: string, text: string, badges: FileBadge[]) => void;
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
  overlays: [],

  setConn: (status, detail) =>
    set((s) => ({
      conn: {
        status,
        attempt: detail?.attempt ?? s.conn.attempt,
        lastEventId: detail?.lastEventId ?? s.conn.lastEventId,
      },
    })),

  // Transition shim: the slash-command registry still calls setView with modal
  // kinds via CommandContext. Map those onto overlay pushes; only the chat kind
  // stays a real route. Removed once commands push overlays directly (Phase 5).
  setView: (view) =>
    set((s) => {
      const overlay = viewToOverlay(view);
      if (!overlay) return { view };
      const already = s.overlays.some(
        (e) =>
          e.kind === overlay.kind &&
          (e.kind !== "permission" || e.askId === (overlay as { askId?: string }).askId),
      );
      return already ? {} : { overlays: s.overlays.concat(overlay) };
    }),
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

  addUserMessage: (sessionId, messageId, text, badges) =>
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
        // The SSE `message_created` echo can arrive BEFORE this optimistic add
        // (the server publishes the event before returning the prompt ack), and
        // it inserts an empty placeholder with a hardcoded role:"assistant".
        // Upgrade that placeholder in place rather than no-op'ing, so the user
        // text + badges survive regardless of which side wins the race. Safe:
        // the ack id is the user message's own id (the assistant reply gets a
        // different id), and user parts are pre-supplied, never streamed, so no
        // real streamed content is clobbered.
        const next = list.slice();
        next[idx] = userMsg;
        return { messages: { ...s.messages, [sessionId]: next } };
      }
      return { messages: { ...s.messages, [sessionId]: list.concat(userMsg) } };
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
              // Settle the streamed text buffer into the final text. Reasoning
              // deltas accumulate in `reasoning_buffer` (not `buffer`), so it
              // is preserved here — clearing it would erase a finished
              // reasoning block, leaving the collapsed view with no content.
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
          // Push a permission overlay carrying its askId so the reply targets
          // the exact request. Multiple permissions can be pending at once
          // (pendingPermissions is keyed by askId); a bare overlay kind would
          // make ≥2 concurrent asks ambiguous. Skip a duplicate push if this
          // askId is already on the stack (idempotent re-delivery).
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
          // Resolve BY askId, from anywhere in the stack — not a blind pop.
          // An async resolve must not dismiss whatever overlay happens to be on
          // top (e.g. a help overlay opened above the permission).
          const next = { ...s.pendingPermissions };
          delete next[ev.ask_id];
          return {
            pendingPermissions: next,
            overlays: s.overlays.filter(
              (e) => !(e.kind === "permission" && e.askId === ev.ask_id),
            ),
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

        case "error":
        case "heartbeat":
        default:
          return {};
      }
    }),
}));
