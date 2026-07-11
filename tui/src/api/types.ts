// Hand-rolled DTOs mirroring crates/openlet-protocol — the actual types the
// client + store speak. `npm run codegen` writes `schema.d.ts` as a
// contract-drift reference snapshot (imported by nothing); it does NOT replace
// these. Keep these in sync with openlet_protocol::dto::*.

export type DeltaKind = "text" | "reasoning" | "tool_args";

export type SessionStatus = "idle" | "running" | "errored" | "cancelled";

export type PermissionMode = "read_only" | "workspace_write" | "danger";

export interface UsageDto {
  input_tokens: number;
  output_tokens: number;
  cached_input_tokens: number;
  cache_write_tokens: number;
  reasoning_tokens: number;
}

export interface AgentDto {
  id: string;
  // Server emits `display_name` (openlet_protocol::dto::AgentDto), not `name`.
  display_name: string;
  description?: string | null;
  // Effective serve model for this agent's turns (config.default_model).
  // Always present from the server; optional here for forward-compat.
  model?: string | null;
  // Total token budget of the agent's model — denominator for the context bar.
  // Optional here for forward-compat with older servers; degrade to no bar.
  context_window?: number;
  // Fraction of context_window at which auto-compaction fires (e.g. 0.8), so
  // the bar can mark "left until compact" rather than the hard limit.
  compaction_threshold?: number;
}

export interface SessionDto {
  id: string;
  agent_id: string;
  status: SessionStatus;
  permission_mode: PermissionMode;
  created_at: string;
  updated_at: string;
  cost_decimal_str: string;
}

export interface PartDto {
  id: string;
  message_id: string;
  kind: "text" | "reasoning" | "tool_call" | "tool_result" | "step_finish" | "compaction";
  text?: string;
  tool_name?: string;
  tool_args?: unknown;
  tool_result?: unknown;
  tool_call_id?: string;
  status?: "streaming" | "complete" | "errored";
  reason?: string;
  usage?: UsageDto;
  cost_decimal_str?: string;
  // Present only on kind:"compaction" — the token count that was summarized
  // away, shown on the transcript divider so the user sees the boundary.
  original_token_count?: number;
}

export interface MessageDto {
  id: string;
  session_id: string;
  role: "user" | "assistant" | "tool" | "system";
  parts: PartDto[];
  created_at: string;
}

// GET /v1/session/:id/messages returns the server's tagged-union parts
// verbatim (openlet_protocol::dto::PartDto — serde tag="kind"). Unlike the
// flat streaming PartDto above, each variant carries only its own fields, so
// tool_call/tool_result bodies (name/args/result) arrive intact. This is the
// ONLY path that delivers tool bodies to the UI; the SSE stream carries just
// part ids. Kept as a discriminated union so `serverPartToView` narrows safely.
export type ServerPartDto =
  | { kind: "text"; id: string; text: string }
  | { kind: "reasoning"; id: string; text: string }
  | { kind: "tool_call"; id: string; call_id: string; name: string; args: unknown }
  | { kind: "tool_result"; id: string; call_id: string; ok: boolean; text?: string | null; error?: string | null }
  | { kind: "image"; id: string; artifact_id: string; mime: string; width: number; height: number }
  | { kind: "document"; id: string; artifact_id: string; mime: string; extracted_text?: string | null }
  | { kind: "step_start"; id: string }
  | { kind: "step_finish"; id: string; reason: string }
  | { kind: "compaction"; id: string; summary: string; compacted_message_ids: string[]; original_token_count: number }
  | { kind: "plan"; id: string; plan: string };

export interface ServerMessageDto {
  id: string;
  session_id: string;
  role: "user" | "assistant" | "tool" | "system";
  created_at: string;
  parts: ServerPartDto[];
}

// Mirrors openlet_protocol::dto::PermissionRequestDto. The server currently
// sends only ask_id + permission (a structured string like "file:write:/path"
// or "bash:rm*") + optional reason/timeout. tool_name/diff/bash/patterns are
// NOT emitted yet (the core PermissionRequest carries no such data) — kept
// optional here so the dialog degrades to `permission` as the display key
// rather than rendering `undefined`.
export interface PermissionRequestDto {
  ask_id: string;
  permission: string;
  reason?: string;
  timeout_ms?: number;
  session_id?: string;
  tool_name?: string;
  tool_args?: unknown;
  diff?: { filepath: string; before: string; after: string };
  bash?: { command: string; cwd: string; timeout_ms: number };
  patterns?: string[];
}

// Mirrors openlet_protocol::dto::PermissionReplyDto (serde snake_case). The
// server takes ONLY `decision` + optional `reason`; the rule pattern for an
// always_* decision is derived server-side from the original ask, never from
// client input, so no pattern/scope field is sent.
export type PermissionReplyKind = "allow" | "deny" | "always_allow" | "always_deny";

export interface PermissionReplyDto {
  decision: PermissionReplyKind;
  reason?: string | null;
}

export interface CreateSessionDto {
  agent_id: string;
  permission_mode?: PermissionMode;
}

export interface CreateMessageDto {
  parts: PartDto[];
}

export interface SetModeDto {
  mode: PermissionMode;
}

export interface PromptAckDto {
  message_id: string;
}

// Mirrors openlet_protocol::dto::AskOptionDto — one selectable answer for an
// `ask_user` question. `description` is optional secondary text.
export interface AskOptionDto {
  label: string;
  description?: string | null;
}

// Body for POST /v1/session/:id/question/answer. `selected` carries the picked
// option indices — exactly one for single-select, zero-or-more for multi.
export interface QuestionAnswerDto {
  question_id: string;
  selected: number[];
}

export interface AbortAckDto {
  ok: true;
}

export interface CompactAckDto {
  compacted: boolean;
}

export interface PluginInfoDto {
  id: string;
  name: string;
  version: string;
  capabilities: string[];
  enabled: boolean;
}

// File @-mention DTOs — mirror crates/openlet-server/src/routes/files.rs.
export type FileKindDto = "text" | "image" | "pdf";

export interface FileEntryDto {
  path: string;
  type: FileKindDto;
}

export interface FileListDto {
  files: FileEntryDto[];
}

export interface FileContentDto {
  path: string;
  type: FileKindDto;
  content?: string;
  truncated?: boolean;
  unsupported?: boolean;
}

export type EventDto =
  | { kind: "session_status"; session_id: string; status: SessionStatus; at: string }
  | { kind: "message_created"; session_id: string; message_id: string; at: string }
  | { kind: "part_created"; session_id: string; message_id: string; part_id: string; at: string }
  | {
      kind: "part_delta";
      session_id: string;
      message_id: string;
      part_id: string;
      delta_kind: DeltaKind;
      delta: string;
    }
  | { kind: "part_updated"; session_id: string; message_id: string; part_id: string }
  | {
      kind: "step_finished";
      session_id: string;
      message_id: string;
      reason: string;
      usage?: UsageDto;
      cost_decimal_str?: string;
    }
  | { kind: "permission_asked"; session_id: string; request: PermissionRequestDto }
  | { kind: "permission_resolved"; ask_id: string; decision: string }
  | {
      kind: "question_requested";
      session_id: string;
      question_id: string;
      header: string;
      question: string;
      options: AskOptionDto[];
      multi_select: boolean;
    }
  | { kind: "error"; session_id?: string; code: string; message: string }
  | { kind: "heartbeat" }
  | { kind: "plugin_error"; plugin_id: string; code: string; message: string }
  | { kind: "plan_mode_entered"; session_id: string; at: string }
  | { kind: "plan_mode_exited"; session_id: string; plan: string; at: string };

export type EventName =
  | "session.status"
  | "message.created"
  | "part.created"
  | "part.delta"
  | "part.updated"
  | "step.finished"
  | "permission.asked"
  | "permission.resolved"
  | "question.requested"
  | "plan_mode.entered"
  | "plan_mode.exited"
  | "error"
  | "heartbeat"
  | "plugin.error";
