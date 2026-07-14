import { describe, expect, it } from "vitest";

import { deriveAgentLabel, formatSiblingPosition } from "../src/components/subagent-footer.js";

describe("deriveAgentLabel", () => {
  it("prefers an explicit agent slug over the title", () => {
    expect(deriveAgentLabel("reviewer", "@researcher subagent")).toBe("reviewer");
    expect(deriveAgentLabel("  worker  ", undefined)).toBe("worker");
  });

  it("falls back to the `@slug subagent` title pattern when no agent given", () => {
    expect(deriveAgentLabel(undefined, "@researcher subagent")).toBe("researcher");
    expect(deriveAgentLabel(undefined, "Session: @reviewer subagent (2 of 3)")).toBe("reviewer");
  });

  it("returns 'subagent' when nothing resolves", () => {
    expect(deriveAgentLabel(undefined, undefined)).toBe("subagent");
    expect(deriveAgentLabel("", "")).toBe("subagent");
    expect(deriveAgentLabel(undefined, "plain child session")).toBe("subagent");
  });
});

describe("formatSiblingPosition", () => {
  it("formats a 1-based (i of n) position", () => {
    expect(formatSiblingPosition(0, 3)).toBe("(1 of 3)");
    expect(formatSiblingPosition(2, 3)).toBe("(3 of 3)");
  });

  it("returns empty string when the index is out of range or total is zero", () => {
    expect(formatSiblingPosition(0, 0)).toBe("");
    expect(formatSiblingPosition(-1, 3)).toBe("");
    expect(formatSiblingPosition(5, 3)).toBe("");
  });
});
