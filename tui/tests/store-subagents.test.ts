import { describe, expect, it, beforeEach } from "vitest";

import { useStore } from "../src/store/index.js";

import type { EventDto } from "../src/api/types.js";

// Reset the subagents slice between tests (the store is a module singleton).
beforeEach(() => {
  useStore.setState({ subagents: {} });
});

describe("store subagents slice", () => {
  it("spawned → progress → settled ends terminal with accumulated output", () => {
    const s = useStore.getState();
    const task = "task-1";
    s.applyEvent({
      kind: "subagent_spawned",
      task_id: task,
      parent_session_id: "parent-1",
      subagent_type: "researcher",
    });
    s.applyEvent({
      kind: "subagent_progress",
      task_id: task,
      parent_session_id: "parent-1",
      delta: "partial ",
    });
    s.applyEvent({
      kind: "subagent_progress",
      task_id: task,
      parent_session_id: "parent-1",
      delta: "result",
    });
    s.applyEvent({
      kind: "subagent_settled",
      task_id: task,
      parent_session_id: "parent-1",
      output: "final result",
      cost_usd: "0.0200",
    });
    const row = useStore.getState().subagents[task];
    expect(row?.status).toBe("finished");
    expect(row?.agent).toBe("researcher");
    // Non-promoted settled carries its final output.
    expect(row?.output).toBe("final result");
  });

  it("promoted task keeps its progress tail (settled frame carries no output)", () => {
    const s = useStore.getState();
    const task = "task-2";
    s.applyEvent({
      kind: "subagent_spawned",
      task_id: task,
      parent_session_id: "parent-1",
      subagent_type: "worker",
    });
    s.applyEvent({
      kind: "subagent_progress",
      task_id: task,
      parent_session_id: "parent-1",
      delta: "in-progress tail",
    });
    s.applyEvent({ kind: "subagent_promoted", task_id: task, parent_session_id: "parent-1" });
    // Promoted settle: output is delivered via the injected parent turn, so the
    // frame's output is empty. The block must keep the prior tail.
    s.applyEvent({
      kind: "subagent_settled",
      task_id: task,
      parent_session_id: "parent-1",
      output: "",
      cost_usd: "0.0100",
    });
    const row = useStore.getState().subagents[task];
    expect(row?.promoted).toBe(true);
    expect(row?.status).toBe("finished");
    expect(row?.output).toBe("in-progress tail");
  });

  it("ignores subagent_message / subagent_roster in the core slice (Phase 6 owns them)", () => {
    const s = useStore.getState();
    s.applyEvent({
      kind: "subagent_roster",
      root_session_id: "root-1",
      entries: [{ name: "reviewer", task_id: "t-9", generation: 1 }],
    });
    s.applyEvent({
      kind: "subagent_message",
      task_id: "t-9",
      parent_session_id: "parent-1",
      from: "a",
      to: "reviewer",
    });
    // No subagents-slice rows created by these frames in Phase 5.
    expect(Object.keys(useStore.getState().subagents)).toHaveLength(0);
  });
});
