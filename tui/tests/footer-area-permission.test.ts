import { describe, expect, it } from "vitest";

import { findSessionPermission } from "../src/components/permission-footer-selection.js";

describe("footer permission selection", () => {
  it("only blocks the session that owns the permission ask", () => {
    const pending = {
      "ask-a": {
        ask_id: "ask-a",
        session_id: "session-a",
        permission: "bash:rm -rf build",
      },
    };

    expect(findSessionPermission(pending, "session-a")?.ask_id).toBe("ask-a");
    expect(findSessionPermission(pending, "session-b")).toBeUndefined();
    expect(findSessionPermission(pending, null)).toBeUndefined();
  });
});
