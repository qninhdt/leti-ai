import { describe, expect, it } from "vitest";

import { parseSubagentCall } from "../src/components/tool-subagent-parse.js";

describe("parseSubagentCall", () => {
  it("extracts agent + objective from args", () => {
    const parsed = parseSubagentCall({ subagent_type: "reviewer", objective: "check the diff" });
    expect(parsed?.agent).toBe("reviewer");
    expect(parsed?.objective).toBe("check the diff");
    expect(parsed?.background).toBe(false);
  });

  it("reads background flag + result fields", () => {
    const parsed = parseSubagentCall(
      { subagent_type: "worker", objective: "do it", background: true },
      { task_id: "t-1", status: "running", cost_usd: "0.0100" },
    );
    expect(parsed?.agent).toBe("worker");
    expect(parsed?.background).toBe(true);
    expect(parsed?.taskId).toBe("t-1");
    expect(parsed?.status).toBe("running");
    expect(parsed?.cost).toBe("0.0100");
  });

  it("tolerates a JSON-string args body", () => {
    const parsed = parseSubagentCall(
      JSON.stringify({ subagent_type: "researcher", objective: "dig" }),
    );
    expect(parsed?.agent).toBe("researcher");
  });

  it("uses general when subagent_type is omitted", () => {
    const parsed = parseSubagentCall({ objective: "survey the project" });
    expect(parsed?.agent).toBe("general");
    expect(parsed?.objective).toBe("survey the project");
  });

  it("returns null on a non-object args body", () => {
    expect(parseSubagentCall("not json")).toBeNull();
    expect(parseSubagentCall(42)).toBeNull();
  });
});
