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

  it("permission_asked sets pending + pushes a permission overlay carrying askId", () => {
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
    const top = state.overlays[state.overlays.length - 1];
    expect(top).toEqual({ kind: "permission", askId: "ask-1" });
  });

  it("permission_resolved clears pending + removes the matching overlay by askId", () => {
    useStore.getState().applyEvent({ kind: "permission_resolved", ask_id: "ask-1", decision: "allow" });
    const state = useStore.getState();
    expect(state.pendingPermissions["ask-1"]).toBeUndefined();
    expect(state.overlays.some((e) => e.kind === "permission" && e.askId === "ask-1")).toBe(false);
  });

  it("resolves the correct permission when two are pending (no wrong-overlay dismissal)", () => {
    const s = useStore.getState();
    const ask = (id: string): EventDto => ({
      kind: "permission_asked",
      session_id: "sid-5",
      request: { ask_id: id, session_id: "sid-5", permission: "edit:foo", tool_name: "edit" },
    });
    s.applyEvent(ask("ask-a"));
    s.applyEvent(ask("ask-b"));
    // Resolve the FIRST (lower in the stack) — a blind top-of-stack pop would
    // wrongly drop ask-b's overlay instead.
    s.applyEvent({ kind: "permission_resolved", ask_id: "ask-a", decision: "allow" });
    const overlays = useStore.getState().overlays;
    expect(overlays.some((e) => e.kind === "permission" && e.askId === "ask-a")).toBe(false);
    expect(overlays.some((e) => e.kind === "permission" && e.askId === "ask-b")).toBe(true);
  });
});
