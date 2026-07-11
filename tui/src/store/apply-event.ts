// The SSE reducer. Every server-sent event frame routes through applyEvent —
// the single mutation point for the store. Kept a PURE function `(state, ev) =>
// Partial<State>` so the store's `set` call stays a thin wrapper (mirrors the
// reducers.ts / message-hydration.ts split in this dir). Snake-case kinds match
// EventDto from crates/openlet-protocol after axum's serde
// rename_all="snake_case". Arms are grouped by domain (parts, session,
// overlays/asks) but the logic of each is byte-identical to its inline origin.

import { upsertPartInMessage, updateMessageById, updatePartById } from "./reducers.js";

import type { EventDto } from "../api/types.js";
import type { MessageView, PendingQuestion, State } from "./types.js";

// Sum two 4-decimal USD cost strings. A NaN parse falls back to the prior
// total so a malformed delta never zeroes the displayed cost.
function addCostStr(prev: string | undefined, delta: string): string {
  const a = Number.parseFloat(prev ?? "0");
  const b = Number.parseFloat(delta);
  const base = Number.isNaN(a) ? 0 : a;
  if (Number.isNaN(b)) return base.toFixed(4);
  return (base + b).toFixed(4);
}

export function applyEvent(s: State, ev: EventDto): Partial<State> {
  switch (ev.kind) {
    // --- session lifecycle -------------------------------------------------
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

    // --- message + part streaming ------------------------------------------
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

    // --- overlays: permissions + questions ---------------------------------
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

    // --- plugins + plan mode + errors --------------------------------------
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
}
