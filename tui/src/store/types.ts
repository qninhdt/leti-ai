// Store type definitions. Shared by the store factory and UI components.

import type { ConnState } from "../api/sse.js";
import type { FileBadge } from "../services/attachment-embedder.js";
import type {
  AgentDto,
  AskOptionDto,
  EventDto,
  MessageDto,
  PartDto,
  PermissionRequestDto,
  PluginInfoDto,
  ServerMessageDto,
  SessionDto,
} from "../api/types.js";

// Overlay stack entries. Each carries the payload its dialog needs to resolve
// itself — notably the permission entry MUST carry `askId` so a reply targets
// the exact request, even with multiple permissions pending (pendingPermissions
// is keyed by askId). A bare kind would make >=2 concurrent asks ambiguous.
export type OverlayEntry =
  | { kind: "permission"; askId: string }
  | { kind: "question"; questionId: string }
  | { kind: "agent_picker" }
  | { kind: "session_picker" }
  | { kind: "help" }
  | { kind: "plugins" }
  | { kind: "command_palette" };

export type OverlayKind = OverlayEntry["kind"];

export interface PartView extends PartDto {
  buffer: string;
  reasoning_buffer: string;
}

export interface MessageView extends MessageDto {
  parts: PartView[];
  step_finish?: { reason: string; usage_total?: number; cost?: string; context_tokens?: number };
  /// File-mention badge chips for a user message. Carried on the optimistic
  /// user message and preserved when the SSE echo arrives.
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

// A pending `ask_user` question awaiting the user's selection. Mirrors the
// `question_requested` SSE frame; keyed in the store by `question_id`. The
// session stays `running` (the tool suspends server-side) until answered, so
// the dialog is the ONLY way to unblock the turn.
export interface PendingQuestion {
  session_id: string;
  question_id: string;
  header: string;
  question: string;
  options: AskOptionDto[];
  multi_select: boolean;
}

/// Live view of one subagent task, keyed by `task_id` in the `subagents`
/// slice. Populated by the `subagent.*` SSE frames (Phase 5). `promoted`
/// marks a task whose result is auto-injected into the parent (Phase 3) —
/// for such a task the `settled` frame carries NO output (it re-enters as a
/// normal parent turn), so the block shows a "result delivered" affordance
/// rather than duplicating the output.
export interface SubagentView {
  task_id: string;
  parent_session_id: string;
  /// Agent slug (from the `spawned` frame).
  agent: string;
  /// running | finished | cancelled | failed. Derived from spawned/settled.
  status: "running" | "finished" | "cancelled" | "failed";
  /// Live output tail accumulated from `progress` frames + the terminal
  /// `settled` output (empty for a promoted task — see `promoted`).
  output: string;
  /// 4-decimal USD cost from the `settled` frame.
  cost?: string;
  /// Set by a `subagent.promoted` frame.
  promoted: boolean;
}

// One live named sibling in the roster slice (Phase 6). Fed by the
// `subagent_roster` frame; the data source for the `@mention` typeahead's
// "live sibling" candidates. Keyed under a root session id in the store.
export interface RosterView {
  name: string;
  task_id: string;
  generation: number;
}

// A passive idle-parent notice (Phase 6, red-team Finding 7). When a promoted
// task settles into an IDLE parent, the TUI surfaces this notice — it never
// auto-starts a turn. The user opens it on their next interaction.
export interface IdleNotice {
  task_id: string;
  parent_session_id: string;
  /// Monotonic sequence so the toast host re-triggers on repeats.
  seq: number;
}

export interface State {
  conn: ConnSlice;
  sessions: Record<string, SessionDto>;
  activeSessionId: string | null;
  messages: Record<string, MessageView[]>;
  /// Subagent task views keyed by `task_id`. Fed by the `subagent.*` frames;
  /// consumed by the inline task block + (Phase 6) the background task panel.
  subagents: Record<string, SubagentView>;
  /// Live sibling roster keyed by root session id, inner keyed by handle name
  /// (Phase 6). Fed by `subagent_roster`; the @mention typeahead's live source.
  roster: Record<string, Record<string, RosterView>>;
  /// Passive idle-parent settle notices (Phase 6, Finding 7). Appended when a
  /// promoted task settles into an idle parent; rendered as a toast, never a
  /// turn-start.
  idleNotices: IdleNotice[];
  agents: AgentDto[];
  plugins: PluginInfoDto[];
  pluginErrors: PluginErrorView[];
  pendingPermissions: Record<string, PermissionRequestDto>;
  /// Pending `ask_user` questions keyed by question_id. Populated by the
  /// `question_requested` SSE frame; drained when the dialog posts an answer
  /// (optimistically) or the turn settles.
  pendingQuestions: Record<string, PendingQuestion>;
  /// Last client-side error (failed prompt/command/session call). Surfaced
  /// as a banner so an async failure in the non-async input handler is
  /// visible to the user instead of becoming an unhandled rejection.
  clientError: string | null;
  /// Transient informational notice (e.g. "/compact" acknowledgment). Unlike
  /// clientError this is not a failure and auto-dismisses; it exists so a
  /// command that only does async work has a way to confirm it ran. Carries a
  /// monotonic `seq` so the toast host re-triggers even on an identical
  /// message.
  notice: { text: string; seq: number } | null;
  /// Per-session plan-mode flag. Latched by `plan_mode_entered`,
  /// cleared by `plan_mode_exited`.
  planMode: Record<string, boolean>;
  /// Overlay stack rendered atop the active route (dialogs, pickers, command
  /// palette). Top of stack = last element.
  overlays: OverlayEntry[];
  applyEvent: (ev: EventDto) => void;
  setConn: (status: ConnState, detail?: { attempt?: number; lastEventId?: number }) => void;
  pushOverlay: (entry: OverlayEntry) => void;
  popOverlay: () => void;
  removeOverlay: (predicate: (entry: OverlayEntry) => boolean) => void;
  clearOverlays: () => void;
  setAgents: (agents: AgentDto[]) => void;
  setPlugins: (plugins: PluginInfoDto[]) => void;
  setSessions: (sessions: SessionDto[]) => void;
  setActiveSession: (id: string | null) => void;
  setClientError: (message: string | null) => void;
  /// Show a transient info notice. Bumps `seq` each call so repeated identical
  /// messages still re-trigger the auto-dismissing toast.
  setNotice: (text: string) => void;
  /// Append an optimistic user message (role:"user") carrying the raw text and
  /// file-mention badges. The SSE stream never produces a user message, so the
  /// TUI must add it itself.
  addUserMessage: (sessionId: string, messageId: string, text: string, badges: FileBadge[]) => void;
  /// Merge server-authoritative message bodies (from GET /messages) into the
  /// session's view. This is the only path that delivers tool call
  /// name/args/results to the UI — the SSE stream carries part ids only.
  hydrateSession: (sessionId: string, messages: ServerMessageDto[]) => void;
  /// Remove a pending question + its overlay once answered/cancelled. The
  /// server has no "question_resolved" event, so the dialog calls this itself
  /// after POSTing the answer (optimistic dismiss).
  clearQuestion: (questionId: string) => void;
}
