// Hand-rolled DTOs mirroring crates/openlet-protocol. Replaced at runtime by
// `npm run codegen` which writes openapi-fetch's `paths` map to schema.d.ts.
// Keep these in sync with openlet_protocol::dto::*.

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
  kind: "text" | "reasoning" | "tool_call" | "tool_result" | "step_finish";
  text?: string;
  tool_name?: string;
  tool_args?: unknown;
  tool_result?: unknown;
  tool_call_id?: string;
  status?: "streaming" | "complete" | "errored";
  reason?: string;
  usage?: UsageDto;
  cost_decimal_str?: string;
}

export interface MessageDto {
  id: string;
  session_id: string;
  role: "user" | "assistant" | "tool" | "system";
  parts: PartDto[];
  created_at: string;
}

export interface PermissionRequestDto {
  ask_id: string;
  session_id: string;
  permission: string;
  tool_name: string;
  tool_args?: unknown;
  diff?: { filepath: string; before: string; after: string };
  bash?: { command: string; cwd: string; timeout_ms: number };
  patterns?: string[];
  reason?: string;
}

export type PermissionReplyKind = "allow" | "deny" | "always" | "never";

export interface PermissionReplyDto {
  reply: PermissionReplyKind;
  pattern?: string | null;
  feedback?: string | null;
  scope?: "session" | "workspace" | "agent";
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

export interface AbortAckDto {
  ok: true;
}

export interface ErrorDto {
  code: string;
  message: string;
}

export interface PluginInfoDto {
  id: string;
  name: string;
  version: string;
  capabilities: string[];
  enabled: boolean;
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
  | "plan_mode.entered"
  | "plan_mode.exited"
  | "error"
  | "heartbeat"
  | "plugin.error";
