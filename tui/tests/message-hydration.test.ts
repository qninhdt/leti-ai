import { describe, expect, it } from "vitest";

import { hydrateMessages, serverPartToView } from "../src/store/message-hydration.js";

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

  it("surfaces a compaction marker with its folded token count", () => {
    const v = serverPartToView({
      kind: "compaction",
      id: "p1",
      summary: "earlier turns summarized",
      compacted_message_ids: ["m0", "m1"],
      original_token_count: 4200,
    });
    expect(v).not.toBeNull();
    expect(v?.kind).toBe("compaction");
    expect(v?.original_token_count).toBe(4200);
  });
});

describe("hydrateMessages", () => {
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

  it("hides messages a compaction superseded but keeps the marker's divider", () => {
    // GET /messages returns the RAW append-only log: the original turns, the
    // synthetic "Summarize…" user request, and the raw summary assistant
    // message — the last two are superseded by the compaction marker and must
    // not render as stray turns beside the divider.
    const server: ServerMessageDto[] = [
      { id: "u0", session_id: "s1", role: "user", created_at: "t", parts: [{ kind: "text", id: "u0p", text: "old question" }] },
      { id: "synth", session_id: "s1", role: "user", created_at: "t", parts: [{ kind: "text", id: "sp", text: "Summarize the conversation history above." }] },
      { id: "sum", session_id: "s1", role: "assistant", created_at: "t", parts: [{ kind: "text", id: "sump", text: "Goal: …" }] },
      {
        id: "comp",
        session_id: "s1",
        role: "assistant",
        created_at: "t",
        parts: [{ kind: "compaction", id: "cp", summary: "Goal: …", compacted_message_ids: ["u0", "synth", "sum"], original_token_count: 3200 }],
      },
      { id: "u1", session_id: "s1", role: "user", created_at: "t", parts: [{ kind: "text", id: "u1p", text: "next question" }] },
    ];
    const out = hydrateMessages([], server);
    // Superseded turns dropped; marker + surrounding real turns survive.
    expect(out.map((m) => m.id)).toEqual(["comp", "u1"]);
    expect(out[0]!.parts[0]!.kind).toBe("compaction");
    expect(out[0]!.parts[0]!.original_token_count).toBe(3200);
  });

  it("keeps superseded call+result together (no orphaned result) when dropped", () => {
    // A superseded assistant turn that issued a tool call: filtering the whole
    // message must take its tool result with it, not strand the result as an
    // orphan standalone row.
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
    expect(out.map((m) => m.id)).toEqual(["comp"]);
  });
});
