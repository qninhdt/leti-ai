import { beforeEach, describe, expect, it } from "vitest";

import { useStore } from "../src/store/index.js";

import type { EventDto } from "../src/api/types.js";

// Reset the shared store slices this suite touches before each test.
beforeEach(() => {
  useStore.setState({ roster: {}, idleNotices: [], subagents: {}, sessions: {} });
});

describe("store subagent_roster", () => {
  it("populates the per-root roster slice keyed by name", () => {
    const s = useStore.getState();
    s.applyEvent({
      kind: "subagent_roster",
      root_session_id: "root-1",
      entries: [
        { name: "reviewer", task_id: "t-1", generation: 1 },
        { name: "reviewer#2", task_id: "t-2", generation: 2 },
      ],
    });
    const roster = useStore.getState().roster["root-1"]!;
    expect(Object.keys(roster).sort()).toEqual(["reviewer", "reviewer#2"]);
    expect(roster["reviewer"]?.task_id).toBe("t-1");
  });

  it("replaces the snapshot so a departed sibling disappears", () => {
    const s = useStore.getState();
    const frame = (entries: { name: string; task_id: string; generation: number }[]): EventDto => ({
      kind: "subagent_roster",
      root_session_id: "root-2",
      entries,
    });
    s.applyEvent(frame([
      { name: "a", task_id: "t-a", generation: 1 },
      { name: "b", task_id: "t-b", generation: 2 },
    ]));
    // `b` finalized → next snapshot omits it.
    s.applyEvent(frame([{ name: "a", task_id: "t-a", generation: 1 }]));
    const roster = useStore.getState().roster["root-2"]!;
    expect(roster["b"]).toBeUndefined();
    expect(roster["a"]).toBeDefined();
  });

  it("a gen bump replaces the stale entry (name rebound to a new task)", () => {
    const s = useStore.getState();
    s.applyEvent({
      kind: "subagent_roster",
      root_session_id: "root-3",
      entries: [{ name: "worker", task_id: "t-old", generation: 5 }],
    });
    s.applyEvent({
      kind: "subagent_roster",
      root_session_id: "root-3",
      entries: [{ name: "worker", task_id: "t-new", generation: 9 }],
    });
    const entry = useStore.getState().roster["root-3"]!["worker"]!;
    expect(entry.task_id).toBe("t-new");
    expect(entry.generation).toBe(9);
  });
});
