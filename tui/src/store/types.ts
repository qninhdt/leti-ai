// Store type definitions. Shared by the store factory and UI components.

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

// Overlay stack entries. Each carries the payload its dialog needs to resolve
// itself — notably the permission entry MUST carry `askId` so a reply targets
// the exact request, even with multiple permissions pending (pendingPermissions
// is keyed by askId). A bare kind would make >=2 concurrent asks ambiguous.
export type OverlayEntry =
  | { kind: "permission"; askId: string }
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
  step_finish?: { reason: string; usage_total?: number; cost?: string };
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
  /// visible to the user instead of becoming an unhandled rejection.
  clientError: string | null;
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
  /// Append an optimistic user message (role:"user") carrying the raw text and
  /// file-mention badges. The SSE stream never produces a user message, so the
  /// TUI must add it itself.
  addUserMessage: (sessionId: string, messageId: string, text: string, badges: FileBadge[]) => void;
}
