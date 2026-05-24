import { describe, expect, it } from "vitest";

import { complete, findCommand, commands } from "../src/commands/registry.js";

describe("commands/registry", () => {
  it("exposes the eight required commands", () => {
    const names = commands.map((c) => c.name).sort();
    expect(names).toEqual(["agents", "cancel", "danger", "help", "new", "plugins", "quit", "sessions"]);
  });

  it("findCommand resolves aliases", () => {
    expect(findCommand("/exit")?.name).toBe("quit");
    expect(findCommand("resume")?.name).toBe("sessions");
  });

  it("complete returns prefix matches", () => {
    const out = complete("/ag");
    expect(out.map((s) => s.display)).toContain("/agents");
  });
});
