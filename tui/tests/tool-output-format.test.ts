import { describe, expect, it } from "vitest";

import { formatToolOutput } from "../src/components/tool-output-format.js";

describe("components/tool-output-format", () => {
  it("bash shows stdout, not the JSON envelope", () => {
    const raw = JSON.stringify({
      stdout: "a.txt\nb.txt\n",
      stderr: "",
      exit_code: 0,
      timed_out: false,
      stdout_truncated: false,
      stderr_truncated: false,
    });
    expect(formatToolOutput("bash", raw)).toBe("a.txt\nb.txt");
  });

  it("bash appends exit code on failure", () => {
    const raw = JSON.stringify({ stdout: "", stderr: "boom", exit_code: 1, timed_out: false });
    expect(formatToolOutput("bash", raw)).toBe("boom\n[exit 1]");
  });

  it("read returns file content", () => {
    const raw = JSON.stringify({ path: "src/main.py", content: "1: def hello():\n2:  pass\n", line_count: 2 });
    expect(formatToolOutput("read", raw)).toBe("1: def hello():\n2:  pass\n");
  });

  it("list renders names with dir slash", () => {
    const raw = JSON.stringify({
      path: ".",
      entries: [
        { name: "src", kind: "dir", size: null },
        { name: "a.txt", kind: "file", size: 3 },
      ],
      truncated: false,
    });
    expect(formatToolOutput("list", raw)).toBe("src/\na.txt");
  });

  it("grep renders path:line: text", () => {
    const raw = JSON.stringify({
      hits: [{ path: "a.rs", line: 12, text: "fn main" }],
      truncated: false,
    });
    expect(formatToolOutput("grep", raw)).toBe("a.rs:12: fn main");
  });

  it("glob renders match list", () => {
    const raw = JSON.stringify({ matches: ["a.ts", "b.ts"], truncated: false });
    expect(formatToolOutput("glob", raw)).toBe("a.ts\nb.ts");
  });

  it("passes through plain error strings unchanged", () => {
    expect(formatToolOutput("bash", "permission denied")).toBe("permission denied");
  });

  it("passes through unknown tool JSON unchanged", () => {
    const raw = JSON.stringify({ foo: 1 });
    expect(formatToolOutput("mystery", raw)).toBe(raw);
  });
});
