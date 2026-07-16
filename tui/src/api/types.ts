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
  parent_session_id?: string | null;
  status: SessionStatus;
  permission_mode: PermissionMode;
  created_at: string;
  updated_at: string;
  cost_decimal_str: string;
}

export interface PartDto {
  id: string;
  message_id: string;
  kind: "text" | "reasoning" | "tool_call" | "tool_result" | "step_finish" | "compaction" | "runtime_reminder";
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
  // Present only on kind:"runtime_reminder" — harness-authored context that
  // projects to the model but must NEVER render as a human user bubble.
  // `kind` above is the Part discriminator; `reminder_kind` is the closed
  // ReminderKind provenance tag (execution_constraint, task_state, etc.).
  reminder_kind?:
    | "execution_constraint"
    | "task_state"
    | "workspace_delta"
    | "background_task_settled"
    | "compaction_recovery"
    | "runtime_limit"
    | "exceptional_outcome";
  stable_key?: string;
  content?: string;
  projection_epoch?: number;
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
  | { kind: "compaction_request"; id: string; state: "pending" | "committed" | "failed"; summary_message_id?: string }
  | { kind: "plan"; id: string; plan: string }
  | {
      kind: "runtime_reminder";
      id: string;
      reminder_kind:
        | "execution_constraint"
        | "task_state"
        | "workspace_delta"
        | "background_task_settled"
        | "compaction_recovery"
        | "runtime_limit"
        | "exceptional_outcome";
      stable_key: string;
      content: string;
      projection_epoch: number;
    };

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

// Mirrors `openlet_protocol::dto::TodoItemDto`. `todo_updated` carries the
// authoritative full-overwrite snapshot after the server confirms its atomic
// artifact persist, so the sidebar can update before message hydration lands.
export interface TodoItemDto {
  content: string;
  status: "pending" | "in_progress" | "completed";
  priority: "high" | "medium" | "low";
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

export interface BackgroundTaskAckDto {
  task_id: string;
  status: string;
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
  | { kind: "plan_mode_exited"; session_id: string; plan: string; at: string }
  | { kind: "todo_updated"; session_id: string; items: TodoItemDto[] }
  // Subagent frames (Phases 2-4). Names match the Phase-2 rename
  // (spawned/progress/settled), plus message/roster
  // (Phase 4). `parent_session_id` routes each to the parent's per-session
  // stream; `subagent.roster` carries `root_session_id` instead.
  | {
      kind: "subagent_spawned";
      task_id: string;
      tool_call_id: string;
      child_session_id: string;
      parent_session_id: string;
      subagent_type: string;
      objective: string;
      description?: string | null;
      background: boolean;
    }
  | { kind: "subagent_progress"; task_id: string; parent_session_id: string; delta: string }
  | {
      kind: "subagent_settled";
      task_id: string;
      child_session_id: string;
      parent_session_id: string;
      status: string;
      cost_usd?: string | null;
    }
  | {
      kind: "subagent_message";
      task_id: string;
      parent_session_id: string;
      from: string;
      to: string;
    }
  | { kind: "subagent_roster"; root_session_id: string; entries: RosterEntryDto[] };

// One live sibling in a `subagent_roster` frame (Phase 4). Mirrors
// openlet_protocol::dto::RosterEntryDto.
export interface RosterEntryDto {
  name: string;
  task_id: string;
  generation: number;
}

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
  | "plugin.error"
  | "todo.updated"
  | "subagent.spawned"
  | "subagent.progress"
  | "subagent.settled"
  | "subagent.message"
  | "subagent.roster";
