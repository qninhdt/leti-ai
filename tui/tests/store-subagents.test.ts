import { describe, expect, it, beforeEach } from "vitest";

import { useStore } from "../src/store/index.js";

import type { EventDto } from "../src/api/types.js";

// Reset the subagents slice between tests (the store is a module singleton).
beforeEach(() => {
  useStore.setState({ subagents: {} });
});

describe("store subagents slice", () => {
  it("spawned → progress → settled keeps navigation metadata and no output body", () => {
    const s = useStore.getState();
    const task = "task-1";
    s.applyEvent({
      kind: "subagent_spawned",
      task_id: task,
      tool_call_id: "call-1",
      child_session_id: "child-1",
      parent_session_id: "parent-1",
      subagent_type: "researcher",
      objective: "Research it",
      background: false,
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
      child_session_id: "child-1",
      parent_session_id: "parent-1",
      status: "finished",
      cost_usd: "0.0200",
    });
    const row = useStore.getState().subagents[task];
    expect(row?.status).toBe("finished");
    expect(row?.agent).toBe("researcher");
    expect(row?.tool_call_id).toBe("call-1");
    expect(row?.child_session_id).toBe("child-1");
    expect(row?.current_activity).toBe("result");
  });

  it("background task keeps its activity through settlement", () => {
    const s = useStore.getState();
    const task = "task-2";
    s.applyEvent({
      kind: "subagent_spawned",
      task_id: task,
      tool_call_id: "call-2",
      child_session_id: "child-2",
      parent_session_id: "parent-1",
      subagent_type: "worker",
      objective: "Work",
      background: true,
    });
    s.applyEvent({
      kind: "subagent_progress",
      task_id: task,
      parent_session_id: "parent-1",
      delta: "in-progress tail",
    });
    s.applyEvent({
      kind: "subagent_settled",
      task_id: task,
      child_session_id: "child-2",
      parent_session_id: "parent-1",
      status: "finished",
      cost_usd: "0.0100",
    });
    const row = useStore.getState().subagents[task];
    expect(row?.status).toBe("finished");
    expect(row?.current_activity).toBe("in-progress tail");
  });

  it("preserves interrupted as a resumable terminal state", () => {
    const s = useStore.getState();
    s.applyEvent({
      kind: "subagent_settled",
      task_id: "task-interrupted",
      child_session_id: "child-3",
      parent_session_id: "parent-1",
      status: "interrupted",
      cost_usd: null,
    });
    expect(useStore.getState().subagents["task-interrupted"]?.status).toBe("interrupted");
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
