import { describe, expect, it } from "vitest";

import { parseAskUser, selectedLabels } from "../src/components/tool-ask-user-parse.js";

describe("parseAskUser", () => {
  it("extracts header, question, options, and multi_select", () => {
    const spec = parseAskUser({
      header: "color",
      question: "Pick a color",
      multi_select: true,
      options: [
        { label: "red", description: "warm" },
        { label: "blue" },
      ],
    });
    expect(spec.header).toBe("color");
    expect(spec.question).toBe("Pick a color");
    expect(spec.multi_select).toBe(true);
    expect(spec.options).toEqual([
      { label: "red", description: "warm" },
      { label: "blue", description: undefined },
    ]);
  });

  it("degrades gracefully on missing/malformed args", () => {
    expect(parseAskUser(undefined)).toEqual({
      header: "",
      question: "",
      options: [],
      multi_select: false,
    });
    expect(parseAskUser("nope").options).toEqual([]);
    expect(parseAskUser({ options: "not-an-array" }).options).toEqual([]);
  });

  it("skips option entries without a string label", () => {
    const spec = parseAskUser({
      options: [{ label: "ok" }, { description: "no label" }, 42, null],
    });
    expect(spec.options).toEqual([{ label: "ok", description: undefined }]);
  });
});

describe("selectedLabels", () => {
  it("reads selected_labels from the result", () => {
    expect(selectedLabels({ selected: [1], selected_labels: ["blue"] })).toEqual(["blue"]);
  });

  it("returns [] when unresolved or malformed", () => {
    expect(selectedLabels(undefined)).toEqual([]);
    expect(selectedLabels({})).toEqual([]);
    expect(selectedLabels({ selected_labels: "blue" })).toEqual([]);
    expect(selectedLabels({ selected_labels: ["a", 2, "b"] })).toEqual(["a", "b"]);
  });
});
