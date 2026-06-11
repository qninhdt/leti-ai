import { describe, expect, it } from "vitest";

import { activeMention, allMentions } from "../src/utils/mention-parser.js";

describe("mention-parser activeMention", () => {
  it("finds the @query the cursor is inside", () => {
    const buf = "look at @src/ap";
    const span = activeMention(buf, buf.length);
    expect(span?.path).toBe("src/ap");
    expect(span?.start).toBe(8);
    expect(span?.end).toBe(buf.length);
  });

  it("returns null when cursor is not in a mention", () => {
    expect(activeMention("plain text", 5)).toBeNull();
  });

  it("requires @ to start a token (not mid-word like an email)", () => {
    expect(activeMention("foo@bar", 7)).toBeNull();
  });

  it("rejects an absolute-path mention (no completion offered)", () => {
    const buf = "@/etc/passwd";
    expect(activeMention(buf, buf.length)).toBeNull();
  });

  it("rejects a Windows drive-letter absolute mention", () => {
    const buf = "@C:/secrets";
    expect(activeMention(buf, buf.length)).toBeNull();
  });
});

describe("mention-parser allMentions", () => {
  it("extracts every @path token in buffer order", () => {
    const found = allMentions("see @src/a.ts and @README.md please");
    expect(found.map((m) => m.path)).toEqual(["src/a.ts", "README.md"]);
  });

  it("excludes absolute + traversal paths from the resolved set", () => {
    const found = allMentions("@src/ok.ts @/abs @C:/win");
    expect(found.map((m) => m.path)).toEqual(["src/ok.ts"]);
  });

  it("reports a span aligned to the @ for each token", () => {
    const buf = "x @a/b.ts";
    const found = allMentions(buf);
    expect(found[0]?.start).toBe(2);
    expect(buf.slice(found[0]!.start, found[0]!.end)).toBe("@a/b.ts");
  });

  it("returns [] when there are no mentions", () => {
    expect(allMentions("no mentions here")).toEqual([]);
  });
});
