import { describe, expect, it } from "vitest";

import { findStreamSafeBoundary, splitBuffer, normalizeNestedFences } from "../src/utils/markdown-walker.js";

describe("markdown-walker", () => {
  it("returns null when buffer is still inside an unclosed fence", () => {
    const buf = "```js\nconst x = 1;\n";
    expect(findStreamSafeBoundary(buf)).toBeNull();
  });

  it("splits at blank line outside fence", () => {
    const buf = "first paragraph\n\nsecond paragraph";
    const split = splitBuffer(buf);
    expect(split.stable).toContain("first paragraph");
    expect(split.tail).toBe("second paragraph");
  });

  it("does not split mid-fence even on blank line", () => {
    const buf = "```\nline1\n\nline2\n";
    expect(findStreamSafeBoundary(buf)).toBeNull();
  });

  it("splits at closing fence", () => {
    const buf = "```\nline1\nline2\n```\n";
    expect(findStreamSafeBoundary(buf)).not.toBeNull();
  });

  it("normalizeNestedFences is idempotent and does not crash on nested fences", () => {
    const md = "```\n```\ninner\n```\n```\n";
    const out = normalizeNestedFences(md);
    expect(typeof out).toBe("string");
    expect(normalizeNestedFences(out)).toBe(out);
  });
});
