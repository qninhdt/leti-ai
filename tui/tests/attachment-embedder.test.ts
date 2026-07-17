import { describe, expect, it } from "vitest";

import { embedMentions } from "../src/services/attachment-embedder.js";

import type { LetiClient } from "../src/api/client.js";
import type { FileContentDto } from "../src/api/types.js";

// Minimal fake client: only getFileContent is exercised by the embedder.
function fakeClient(map: Record<string, FileContentDto | Error>): LetiClient {
  return {
    getFileContent: async (path: string) => {
      const entry = map[path];
      if (entry instanceof Error) throw entry;
      if (!entry) throw new Error("file not found");
      return entry;
    },
  } as unknown as LetiClient;
}

describe("attachment-embedder", () => {
  it("embeds text content as a fenced block with lang by extension", async () => {
    const client = fakeClient({
      "src/a.ts": { path: "src/a.ts", type: "text", content: "const x = 1;" },
    });
    const { promptSection, badges } = await embedMentions("look @src/a.ts", client);
    expect(promptSection).toContain("@src/a.ts:");
    expect(promptSection).toContain("```ts");
    expect(promptSection).toContain("const x = 1;");
    expect(badges).toHaveLength(1);
    expect(badges[0]).toMatchObject({ path: "src/a.ts", unsupported: false });
  });

  it("badges image/pdf as unsupported with no embedded content", async () => {
    const client = fakeClient({
      "docs/logo.png": { path: "docs/logo.png", type: "image", unsupported: true },
    });
    const { promptSection, badges } = await embedMentions("@docs/logo.png", client);
    expect(promptSection).toBe("");
    expect(badges[0]).toMatchObject({ path: "docs/logo.png", unsupported: true });
  });

  it("records an error badge (no throw) when a file is missing/denied", async () => {
    const client = fakeClient({ ".env": new Error("file not found") });
    const { promptSection, badges } = await embedMentions("@.env", client);
    expect(promptSection).toBe("");
    expect(badges[0]?.unsupported).toBe(true);
    expect(badges[0]?.error).toBeDefined();
  });

  it("never embeds content for an absolute-path mention (parser rejects it first)", async () => {
    const client = fakeClient({});
    const { promptSection, badges } = await embedMentions("@/etc/passwd", client);
    expect(promptSection).toBe("");
    expect(badges).toEqual([]);
  });

  it("dedupes a path mentioned twice into a single fetch + badge", async () => {
    let calls = 0;
    const client = {
      getFileContent: async (path: string) => {
        calls++;
        return { path, type: "text", content: "x" } as FileContentDto;
      },
    } as unknown as LetiClient;
    const { badges } = await embedMentions("@src/a.ts and again @src/a.ts", client);
    expect(calls).toBe(1);
    expect(badges).toHaveLength(1);
  });

  it("returns empty for a buffer with no mentions", async () => {
    const { promptSection, badges } = await embedMentions("no mentions", fakeClient({}));
    expect(promptSection).toBe("");
    expect(badges).toEqual([]);
  });
});
