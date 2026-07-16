import { describe, expect, it } from "vitest";

import { useStore } from "../src/store/index.js";

import type { EventDto } from "../src/api/types.js";

describe("store applyEvent", () => {
  it("creates a message on message_created", () => {
    const s = useStore.getState();
    s.applyEvent({
      kind: "message_created",
      session_id: "sid-1",
      message_id: "mid-1",
      at: new Date().toISOString(),
    });
    expect(useStore.getState().messages["sid-1"]?.length).toBe(1);
  });

  it("appends text via part_delta into per-part buffer", () => {
    const s = useStore.getState();
    const sid = "sid-2";
    const mid = "mid-2";
    const pid = "pid-2";
    s.applyEvent({ kind: "message_created", session_id: sid, message_id: mid, at: "" });
    s.applyEvent({ kind: "part_created", session_id: sid, message_id: mid, part_id: pid, at: "" });
    for (const ch of ["He", "llo", " world"]) {
      const ev: EventDto = {
        kind: "part_delta",
        session_id: sid,
        message_id: mid,
        part_id: pid,
        delta_kind: "text",
        delta: ch,
      };
      s.applyEvent(ev);
    }
    const part = useStore.getState().messages[sid]?.[0]?.parts[0];
    expect(part?.buffer).toBe("Hello world");
  });

  it("part_updated finalizes buffer into text", () => {
    const s = useStore.getState();
    const sid = "sid-3";
    const mid = "mid-3";
    const pid = "pid-3";
    s.applyEvent({ kind: "message_created", session_id: sid, message_id: mid, at: "" });
    s.applyEvent({ kind: "part_created", session_id: sid, message_id: mid, part_id: pid, at: "" });
    s.applyEvent({
      kind: "part_delta",
      session_id: sid,
      message_id: mid,
      part_id: pid,
      delta_kind: "text",
      delta: "done",
    });
    s.applyEvent({ kind: "part_updated", session_id: sid, message_id: mid, part_id: pid });
    const part = useStore.getState().messages[sid]?.[0]?.parts[0];
    expect(part?.text).toBe("done");
    expect(part?.buffer).toBe("");
    expect(part?.status).toBe("complete");
  });

  it("part_updated preserves reasoning_buffer so a finished thought keeps content", () => {
    const s = useStore.getState();
    const sid = "sid-r";
    const mid = "mid-r";
    const pid = "pid-r";
    s.applyEvent({ kind: "message_created", session_id: sid, message_id: mid, at: "" });
    s.applyEvent({ kind: "part_created", session_id: sid, message_id: mid, part_id: pid, at: "" });
    s.applyEvent({
      kind: "part_delta",
      session_id: sid,
      message_id: mid,
      part_id: pid,
      delta_kind: "reasoning",
      delta: "weighing options",
    });
    s.applyEvent({ kind: "part_updated", session_id: sid, message_id: mid, part_id: pid });
    const part = useStore.getState().messages[sid]?.[0]?.parts[0];
    // Reasoning deltas accumulate in reasoning_buffer, not buffer; finalizing
    // must not wipe it or the collapsed "Thought" view renders empty.
    expect(part?.reasoning_buffer).toBe("weighing options");
    expect(part?.status).toBe("complete");
  });

  it("permission_asked queues the request for the footer without opening an overlay", () => {
    const s = useStore.getState();
    s.applyEvent({
      kind: "permission_asked",
      session_id: "sid-4",
      request: {
        ask_id: "ask-1",
        session_id: "sid-4",
        permission: "edit:foo",
        tool_name: "edit",
      },
    });
    const state = useStore.getState();
    expect(state.pendingPermissions["ask-1"]).toBeDefined();
    expect(state.pendingPermissions["ask-1"]?.session_id).toBe("sid-4");
    expect(state.overlays).toEqual([]);
  });

  it("permission_resolved clears the matching footer request by askId", () => {
    useStore.getState().applyEvent({ kind: "permission_resolved", ask_id: "ask-1", decision: "allow" });
    const state = useStore.getState();
    expect(state.pendingPermissions["ask-1"]).toBeUndefined();
  });

  it("resolves the correct permission when two footer requests are pending", () => {
    const s = useStore.getState();
    const ask = (id: string): EventDto => ({
      kind: "permission_asked",
      session_id: "sid-5",
      request: { ask_id: id, session_id: "sid-5", permission: "edit:foo", tool_name: "edit" },
    });
    s.applyEvent(ask("ask-a"));
    s.applyEvent(ask("ask-b"));
    // Resolving one request must not dismiss another queued footer request.
    s.applyEvent({ kind: "permission_resolved", ask_id: "ask-a", decision: "allow" });
    const pending = useStore.getState().pendingPermissions;
    expect(pending["ask-a"]).toBeUndefined();
    expect(pending["ask-b"]?.ask_id).toBe("ask-b");
    expect(useStore.getState().overlays).toEqual([]);
  });

  it("permission_resolved is idempotent so the footer's local reply fallback is safe", () => {
    const s = useStore.getState();
    s.applyEvent({
      kind: "permission_asked",
      session_id: "sid-6",
      request: { ask_id: "ask-c", session_id: "sid-6", permission: "edit:foo", tool_name: "edit" },
    });
    // The footer applies a local resolution on POST success when SSE cannot
    // deliver the authoritative frame; the later real frame must be a no-op,
    // never resurrecting or corrupting state.
    s.applyEvent({ kind: "permission_resolved", ask_id: "ask-c", decision: "allow" });
    expect(useStore.getState().pendingPermissions["ask-c"]).toBeUndefined();
    s.applyEvent({ kind: "permission_resolved", ask_id: "ask-c", decision: "allow" });
    expect(useStore.getState().pendingPermissions["ask-c"]).toBeUndefined();
  });

  it("surfaces a server error event into clientError (not silently dropped)", () => {
    const s = useStore.getState();
    s.applyEvent({
      kind: "error",
      session_id: "sid-err",
      code: "provider_error",
      message: "upstream 403: model not subscribed",
    });
    expect(useStore.getState().clientError).toBe(
      "Agent error: upstream 403: model not subscribed",
    );
  });

  it("falls back to the error code when the message is empty", () => {
    const s = useStore.getState();
    s.applyEvent({ kind: "error", code: "timeout", message: "" });
    expect(useStore.getState().clientError).toBe("Agent error: timeout");
  });

  it("todo_updated replaces the authoritative live checklist, including an empty clear", () => {
    const s = useStore.getState();
    s.applyEvent({
      kind: "todo_updated",
      session_id: "sid-todo",
      items: [
        { content: "implement", status: "in_progress", priority: "high" },
        { content: "verify", status: "pending", priority: "medium" },
      ],
    });
    expect(useStore.getState().todos["sid-todo"]).toEqual([
      { content: "implement", status: "in_progress", priority: "high" },
      { content: "verify", status: "pending", priority: "medium" },
    ]);

    // Full-overwrite semantics: an empty list is a real update, not a
    // missing snapshot that should fall back to historical tool args.
    s.applyEvent({ kind: "todo_updated", session_id: "sid-todo", items: [] });
    expect(useStore.getState().todos["sid-todo"]).toEqual([]);
  });

  it("addUserMessage reconciles an SSE placeholder that arrived first (no dup, role=user, badges kept)", () => {
    const s = useStore.getState();
    const sid = "sid-opt";
    const mid = "mid-opt";
    // SSE echo wins the race: message_created inserts an empty role:"assistant"
    // placeholder for this id BEFORE the optimistic add runs.
    s.applyEvent({ kind: "message_created", session_id: sid, message_id: mid, at: "" });
    s.addUserMessage(sid, mid, "look at @src/app.tsx", [
      { path: "src/app.tsx", kind: "text", unsupported: false, truncated: false },
    ]);
    const list = useStore.getState().messages[sid] ?? [];
    expect(list.length).toBe(1);
    expect(list[0]?.role).toBe("user");
    expect(list[0]?.parts[0]?.text).toBe("look at @src/app.tsx");
    expect(list[0]?.badges?.[0]?.path).toBe("src/app.tsx");
  });

  it("addUserMessage appends when no placeholder exists (add wins the race)", () => {
    const s = useStore.getState();
    const sid = "sid-opt2";
    const mid = "mid-opt2";
    s.addUserMessage(sid, mid, "hello", []);
    // A later echo for the same id must dedupe, not duplicate.
    s.applyEvent({ kind: "message_created", session_id: sid, message_id: mid, at: "" });
    const list = useStore.getState().messages[sid] ?? [];
    expect(list.length).toBe(1);
    expect(list[0]?.role).toBe("user");
  });

  it("question_requested records the pending question + pushes a question overlay", () => {
    const s = useStore.getState();
    s.applyEvent({
      kind: "question_requested",
      session_id: "sid-q",
      question_id: "q-1",
      header: "Pick one",
      question: "Which framework?",
      options: [{ label: "Solid" }, { label: "React", description: "the classic" }],
      multi_select: false,
    });
    const state = useStore.getState();
    expect(state.pendingQuestions["q-1"]).toMatchObject({
      question_id: "q-1",
      multi_select: false,
    });
    expect(state.pendingQuestions["q-1"]?.options).toHaveLength(2);
    const top = state.overlays[state.overlays.length - 1];
    expect(top).toEqual({ kind: "question", questionId: "q-1" });
  });

  it("question_requested is idempotent — a re-emitted frame does not stack a 2nd overlay", () => {
    const s = useStore.getState();
    const ev: EventDto = {
      kind: "question_requested",
      session_id: "sid-q2",
      question_id: "q-2",
      header: "H",
      question: "Q?",
      options: [{ label: "a" }, { label: "b" }],
      multi_select: false,
    };
    s.applyEvent(ev);
    s.applyEvent(ev);
    const count = useStore
      .getState()
      .overlays.filter((e) => e.kind === "question" && e.questionId === "q-2").length;
    expect(count).toBe(1);
  });

  it("clearQuestion drains the pending question + removes its overlay (optimistic answer)", () => {
    const s = useStore.getState();
    s.applyEvent({
      kind: "question_requested",
      session_id: "sid-q3",
      question_id: "q-3",
      header: "H",
      question: "Q?",
      options: [{ label: "a" }, { label: "b" }],
      multi_select: true,
    });
    useStore.getState().clearQuestion("q-3");
    const state = useStore.getState();
    expect(state.pendingQuestions["q-3"]).toBeUndefined();
    expect(state.overlays.some((e) => e.kind === "question" && e.questionId === "q-3")).toBe(false);
  });
});
