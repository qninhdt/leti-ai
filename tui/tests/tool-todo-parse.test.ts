import { describe, expect, it } from "vitest";

import { parseTodos } from "../src/components/tool-todo-parse.js";

describe("parseTodos", () => {
  it("extracts a well-formed list preserving order + status", () => {
    const items = parseTodos({
      todos: [
        { content: "scaffold", status: "completed", priority: "high" },
        { content: "wire api", status: "in_progress", priority: "medium" },
        { content: "tests", status: "pending", priority: "low" },
      ],
    });
    expect(items.map((t) => t.content)).toEqual(["scaffold", "wire api", "tests"]);
    expect(items.map((t) => t.status)).toEqual(["completed", "in_progress", "pending"]);
  });

  it("returns [] for missing/malformed args (never throws)", () => {
    expect(parseTodos(undefined)).toEqual([]);
    expect(parseTodos(null)).toEqual([]);
    expect(parseTodos({})).toEqual([]);
    expect(parseTodos({ todos: "nope" })).toEqual([]);
    expect(parseTodos("string")).toEqual([]);
  });

  it("skips entries without a string content", () => {
    const items = parseTodos({
      todos: [{ status: "pending" }, { content: 42 }, { content: "ok", status: "pending" }],
    });
    expect(items).toHaveLength(1);
    expect(items[0]!.content).toBe("ok");
  });

  it("coerces an unknown status to pending (forward-compatible, item not dropped)", () => {
    const items = parseTodos({ todos: [{ content: "x", status: "blocked" }] });
    expect(items).toHaveLength(1);
    expect(items[0]!.status).toBe("pending");
  });

  it("defaults a missing status to pending", () => {
    const items = parseTodos({ todos: [{ content: "x" }] });
    expect(items[0]!.status).toBe("pending");
  });

  it("coerces a legacy persisted 'cancelled' status to pending (enum shrank 4→3)", () => {
    // `cancelled` was dropped from TodoStatus; an old todos.json holding it
    // must still render rather than break — the unknown-status fallback coerces
    // it to pending.
    const items = parseTodos({ todos: [{ content: "old", status: "cancelled" }] });
    expect(items).toHaveLength(1);
    expect(items[0]!.status).toBe("pending");
  });
});
