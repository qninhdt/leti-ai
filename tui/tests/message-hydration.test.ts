import { describe, expect, it } from "vitest";

import {
  hydrateMessages,
  isRuntimeReminderOnly,
  serverPartToView,
} from "../src/store/message-hydration.js";

import type { ServerMessageDto } from "../src/api/types.js";
import type { MessageView } from "../src/store/types.js";

function assistantWithCall(msgId: string, callId: string, name: string): ServerMessageDto {
  return {
    id: msgId,
    session_id: "s1",
    role: "assistant",
    created_at: "t",
    parts: [{ kind: "tool_call", id: `${msgId}-p`, call_id: callId, name, args: { path: "x" } }],
  };
}

function toolResult(msgId: string, callId: string, text: string): ServerMessageDto {
  return {
    id: msgId,
    session_id: "s1",
    role: "tool",
    created_at: "t",
    parts: [{ kind: "tool_result", id: `${msgId}-p`, call_id: callId, ok: true, text }],
  };
}

describe("serverPartToView", () => {
  it("maps a tool_call to a flat tool_call PartView", () => {
    const v = serverPartToView({ kind: "tool_call", id: "p1", call_id: "c1", name: "write", args: { a: 1 } });
    expect(v).toMatchObject({ kind: "tool_call", tool_name: "write", tool_call_id: "c1", tool_args: { a: 1 } });
  });

  it("maps a successful tool_result to its text body", () => {
    const v = serverPartToView({ kind: "tool_result", id: "p1", call_id: "c1", ok: true, text: "done" });
    expect(v).toMatchObject({ kind: "tool_result", tool_result: "done", status: "complete" });
  });

  it("maps a failed tool_result to its error body with errored status", () => {
    const v = serverPartToView({ kind: "tool_result", id: "p1", call_id: "c1", ok: false, error: "boom" });
    expect(v).toMatchObject({ kind: "tool_result", tool_result: "boom", status: "errored" });
  });

  it("drops non-inline parts (step markers)", () => {
    expect(serverPartToView({ kind: "step_finish", id: "p1", reason: "stop" })).toBeNull();
    expect(serverPartToView({ kind: "step_start", id: "p2" })).toBeNull();
  });

  it("maps a typed compaction request to the timeline divider", () => {
    const v = serverPartToView({
      kind: "compaction_request",
      id: "p1",
      state: "committed",
    });
    expect(v).not.toBeNull();
    expect(v?.kind).toBe("compaction");
    expect(v?.original_token_count).toBe(0);
  });

  it("hides a failed compaction request from the timeline", () => {
    expect(
      serverPartToView({ kind: "compaction_request", id: "p1", state: "failed" }),
    ).toBeNull();
  });

  it("retains a runtime reminder as typed control state", () => {
    const v = serverPartToView({
      kind: "runtime_reminder",
      id: "r1",
      reminder_kind: "execution_constraint",
      stable_key: "mode:read_only",
      content: "read only",
      projection_epoch: 2,
    });
    expect(v).toMatchObject({
      kind: "runtime_reminder",
      reminder_kind: "execution_constraint",
      projection_epoch: 2,
    });
  });
});

describe("hydrateMessages", () => {
  it("keeps reminder-only messages in control state without classifying user text as control", () => {
    const server: ServerMessageDto[] = [
      {
        id: "r1",
        session_id: "s1",
        role: "user",
        created_at: "t",
        parts: [{
          kind: "runtime_reminder",
          id: "rp1",
          reminder_kind: "execution_constraint",
          stable_key: "mode:read_only",
          content: "read only",
          projection_epoch: 0,
        }],
      },
      {
        id: "u1",
        session_id: "s1",
        role: "user",
        created_at: "t",
        parts: [{ kind: "text", id: "up1", text: "<system-reminder>user text</system-reminder>" }],
      },
    ];
    const out = hydrateMessages([], server);
    expect(out).toHaveLength(2);
    expect(isRuntimeReminderOnly(out[0]!)).toBe(true);
    expect(isRuntimeReminderOnly(out[1]!)).toBe(false);
  });

  it("keeps a legacy child objective visible when it has no structural control provenance", () => {
    const server: ServerMessageDto[] = [{
      id: "child-objective",
      session_id: "child-1",
      role: "user",
      created_at: "t",
      parts: [{ kind: "text", id: "objective-text", text: "Inspect the parser and report findings." }],
    }];
    const out = hydrateMessages([], server);
    expect(out).toHaveLength(1);
    expect(out[0]!.parts[0]!.text).toBe("Inspect the parser and report findings.");
    expect(isRuntimeReminderOnly(out[0]!)).toBe(false);
  });

  it("folds a tool result into the assistant message that issued the call", () => {
    const server = [assistantWithCall("m1", "c1", "write"), toolResult("m2", "c1", "ok")];
    const out = hydrateMessages([], server);

    // The tool message is emptied + dropped; result folds into m1 right after the call.
    expect(out).toHaveLength(1);
    expect(out[0]!.id).toBe("m1");
    expect(out[0]!.parts.map((p) => p.kind)).toEqual(["tool_call", "tool_result"]);
    expect(out[0]!.parts[1]).toMatchObject({ tool_result: "ok" });
  });

  it("keeps an orphan tool result as its own message when no call matches", () => {
    const out = hydrateMessages([], [toolResult("m2", "missing", "ok")]);
    expect(out).toHaveLength(1);
    expect(out[0]!.role).toBe("tool");
    expect(out[0]!.parts[0]).toMatchObject({ tool_result: "ok" });
  });

  it("preserves a store-only in-flight message not yet on the server", () => {
    const inflight: MessageView = {
      id: "live",
      session_id: "s1",
      role: "assistant",
      created_at: "t",
      parts: [{ id: "lp", message_id: "", kind: "text", text: "", buffer: "streaming…", reasoning_buffer: "", status: "streaming" }],
    };
    const out = hydrateMessages([inflight], [assistantWithCall("m1", "c1", "write")]);
    expect(out.map((m) => m.id)).toEqual(["m1", "live"]);
  });

  it("keeps the streaming store copy over a stale server copy for the same id", () => {
    const streaming: MessageView = {
      id: "m1",
      session_id: "s1",
      role: "assistant",
      created_at: "t",
      parts: [{ id: "p", message_id: "", kind: "text", text: "", buffer: "live", reasoning_buffer: "", status: "streaming" }],
    };
    const server = [
      { id: "m1", session_id: "s1", role: "assistant" as const, created_at: "t", parts: [] },
    ];
    const out = hydrateMessages([streaming], server);
    expect(out).toHaveLength(1);
    expect(out[0]!.parts[0]!.buffer).toBe("live");
  });

  it("carries optimistic user badges onto the settled server message", () => {
    const withBadge: MessageView = {
      id: "u1",
      session_id: "s1",
      role: "user",
      created_at: "t",
      parts: [{ id: "p", message_id: "", kind: "text", text: "hi", buffer: "", reasoning_buffer: "", status: "complete" }],
      badges: [{ path: "a.ts", kind: "text", unsupported: false, truncated: false }],
    };
    const server: ServerMessageDto[] = [
      { id: "u1", session_id: "s1", role: "user", created_at: "t", parts: [{ kind: "text", id: "p", text: "hi" }] },
    ];
    const out = hydrateMessages([withBadge], server);
    expect(out[0]!.badges).toEqual([{ path: "a.ts", kind: "text", unsupported: false, truncated: false }]);
  });

  it("carries the step_finish usage/cost readout onto the settled assistant message", () => {
    // step_finish is derived from the step_finished SSE event; the server's
    // step_finish part carries only `reason`, so hydration must preserve the
    // store's copy or the footer + context bar lose their token/cost readout
    // the instant the turn settles.
    const streamed: MessageView = {
      id: "a1",
      session_id: "s1",
      role: "assistant",
      created_at: "t",
      parts: [{ id: "p", message_id: "", kind: "text", text: "hi", buffer: "", reasoning_buffer: "", status: "complete" }],
      step_finish: { reason: "end_turn", usage_total: 15, cost: "0.0001", context_tokens: 12 },
    };
    const server: ServerMessageDto[] = [
      {
        id: "a1",
        session_id: "s1",
        role: "assistant",
        created_at: "t",
        parts: [
          { kind: "text", id: "p", text: "hi" },
          { kind: "step_finish", id: "sf", reason: "end_turn" },
        ],
      },
    ];
    const out = hydrateMessages([streamed], server);
    expect(out[0]!.step_finish).toEqual({ reason: "end_turn", usage_total: 15, cost: "0.0001", context_tokens: 12 });
  });

  it("keeps the clean optimistic user text over the @mention-expanded server copy", () => {
    // The store holds what the user TYPED; the server holds that text with
    // embedded @mention file bodies appended (meant for the model). Hydration
    // must not swap the display text for the expanded blob.
    const typed: MessageView = {
      id: "u1",
      session_id: "s1",
      role: "user",
      created_at: "t",
      parts: [{ id: "p", message_id: "", kind: "text", text: "explain @a.ts", buffer: "", reasoning_buffer: "", status: "complete" }],
    };
    const server: ServerMessageDto[] = [
      {
        id: "u1",
        session_id: "s1",
        role: "user",
        created_at: "t",
        parts: [{ kind: "text", id: "p", text: "explain @a.ts\n\n<file a.ts>\n…1000 lines…\n</file>" }],
      },
    ];
    const out = hydrateMessages([typed], server);
    expect(out[0]!.parts[0]!.text).toBe("explain @a.ts");
  });

  it("keeps human history and renders typed divider followed by summary", () => {
    const server: ServerMessageDto[] = [
      { id: "u0", session_id: "s1", role: "user", created_at: "t", parts: [{ kind: "text", id: "u0p", text: "old question" }] },
      { id: "synth", session_id: "s1", role: "user", created_at: "t", parts: [{ kind: "compaction_request", id: "sp", state: "committed" }] },
      { id: "sum", session_id: "s1", role: "assistant", created_at: "t", parts: [{ kind: "text", id: "sump", text: "Goal: …" }, { kind: "compaction", id: "cp", summary: "Goal: …", compacted_message_ids: ["u0"], original_token_count: 3200 }] },
      { id: "u1", session_id: "s1", role: "user", created_at: "t", parts: [{ kind: "text", id: "u1p", text: "next question" }] },
    ];
    const out = hydrateMessages([], server);
    expect(out.map((m) => m.id)).toEqual(["u0", "synth", "sum", "u1"]);
    expect(out[1]!.parts[0]!.kind).toBe("compaction");
    expect(out[2]!.parts[0]!.text).toBe("Goal: …");
  });

  it("suppresses a partial assistant summary owned by a failed attempt", () => {
    const out = hydrateMessages([], [
      {
        id: "request",
        session_id: "s1",
        role: "user",
        created_at: "t",
        parts: [
          {
            kind: "compaction_request",
            id: "request-part",
            state: "failed",
            summary_message_id: "partial",
          },
        ],
      },
      {
        id: "partial",
        session_id: "s1",
        role: "assistant",
        created_at: "t",
        parts: [{ kind: "text", id: "partial-part", text: "half a summary" }],
      },
    ]);
    expect(out).toHaveLength(0);
  });

  it("keeps compacted call/result history in the human timeline", () => {
    const server: ServerMessageDto[] = [
      assistantWithCall("a0", "c0", "read"),
      toolResult("t0", "c0", "file body"),
      {
        id: "comp",
        session_id: "s1",
        role: "assistant",
        created_at: "t",
        parts: [{ kind: "compaction", id: "cp", summary: "s", compacted_message_ids: ["a0", "t0"], original_token_count: 100 }],
      },
    ];
    const out = hydrateMessages([], server);
    expect(out.map((m) => m.id)).toEqual(["a0", "comp"]);
  });
});
