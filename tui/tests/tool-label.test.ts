import { describe, expect, it } from "vitest";

import { toolLabel, toolBlockTitle } from "../src/components/tool-label.js";

describe("toolLabel", () => {
  it("surfaces the file path for read/write/edit", () => {
    expect(toolLabel("read", { path: "src/a.ts" })).toBe("src/a.ts");
    expect(toolLabel("write", { path: "hello.txt", content: "hi" })).toBe("hello.txt");
    expect(toolLabel("edit", { path: "b.ts", find: "x", replace: "y" })).toBe("b.ts");
  });

  it("surfaces the command for bash", () => {
    expect(toolLabel("bash", { command: "ls -l" })).toBe("ls -l");
  });

  it("surfaces the pattern for glob and grep, with grep scope", () => {
    expect(toolLabel("glob", { pattern: "**/*.ts" })).toBe("**/*.ts");
    expect(toolLabel("grep", { pattern: "TODO", path_glob: "src/**" })).toBe("TODO in src/**");
  });

  it("defaults list to the cwd dot when no path", () => {
    expect(toolLabel("list", {})).toBe(".");
  });

  it("falls back to a compact JSON summary for unknown tools", () => {
    expect(toolLabel("mystery", { a: 1 })).toBe('{"a":1}');
  });

  it("is case-insensitive on tool name", () => {
    expect(toolLabel("Read", { path: "x" })).toBe("x");
  });
});

describe("toolBlockTitle", () => {
  it("renders a write as 'Wrote <path>'", () => {
    expect(toolBlockTitle("write", { path: "hello.txt" })).toBe("Wrote hello.txt");
  });

  it("renders a bash as '$ <command>'", () => {
    expect(toolBlockTitle("bash", { command: "ls" })).toBe("$ ls");
  });

  it("renders an edit as 'Edited <path>'", () => {
    expect(toolBlockTitle("edit", { path: "b.ts" })).toBe("Edited b.ts");
  });

  it("degrades to the bare tool name when args are empty", () => {
    expect(toolBlockTitle("write", {})).toBe("Write");
  });
});
