import { describe, expect, it } from "vitest";

import { mentionCandidates } from "../src/components/subagent-mention-typeahead.js";
import { groupSubagents } from "../src/components/subagent-panel-group.js";

import type { AgentDto } from "../src/api/types.js";
import type { RosterView, SubagentView } from "../src/store/types.js";

function agent(name: string, description?: string): AgentDto {
  return { id: name, display_name: name, description };
}

function roster(...names: string[]): Record<string, RosterView> {
  const out: Record<string, RosterView> = {};
  names.forEach((name, i) => {
    out[name] = { name, task_id: `t-${i}`, generation: i + 1 };
  });
  return out;
}

describe("mentionCandidates", () => {
  it("returns static agents + live siblings, deduped, live first", () => {
    const cands = mentionCandidates(
      "re",
      [agent("researcher", "digs"), agent("reviewer")],
      roster("reviewer#2"),
    );
    // Live sibling first, then static agents; reviewer#2 is live-annotated.
    expect(cands[0]).toEqual({ name: "reviewer#2", kind: "live", detail: "● running" });
    expect(cands.some((c) => c.name === "researcher" && c.kind === "agent")).toBe(true);
    expect(cands.some((c) => c.name === "reviewer" && c.kind === "agent")).toBe(true);
  });

  it("a live sibling shadows a same-name static agent (dedup, live wins)", () => {
    const cands = mentionCandidates("", [agent("worker")], roster("worker"));
    const workers = cands.filter((c) => c.name === "worker");
    expect(workers).toHaveLength(1);
    expect(workers[0]?.kind).toBe("live");
  });

  it("prefix-filters case-insensitively; empty query matches all", () => {
    const all = mentionCandidates("", [agent("Alpha"), agent("Beta")], {});
    expect(all).toHaveLength(2);
    const filtered = mentionCandidates("al", [agent("Alpha"), agent("Beta")], {});
    expect(filtered.map((c) => c.name)).toEqual(["Alpha"]);
  });
});

describe("groupSubagents", () => {
  const row = (
    task_id: string,
    status: SubagentView["status"],
    promoted: boolean,
  ): SubagentView => ({
    task_id,
    parent_session_id: "p",
    agent: "worker",
    status,
    output: "",
    promoted,
  });

  it("partitions running / promoted / settled by state", () => {
    const groups = groupSubagents({
      a: row("a", "running", false),
      b: row("b", "running", true),
      c: row("c", "finished", false),
      d: row("d", "failed", true),
    });
    expect(groups.running.map((r) => r.task_id)).toEqual(["a"]);
    expect(groups.promoted.map((r) => r.task_id)).toEqual(["b"]);
    // Terminal tasks land in settled regardless of promotion.
    expect(groups.settled.map((r) => r.task_id).sort()).toEqual(["c", "d"]);
  });

  it("empty slice yields three empty buckets", () => {
    const groups = groupSubagents({});
    expect(groups.running).toEqual([]);
    expect(groups.promoted).toEqual([]);
    expect(groups.settled).toEqual([]);
  });
});
