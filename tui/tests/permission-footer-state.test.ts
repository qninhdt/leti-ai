import { describe, expect, it } from "vitest";

import {
  activatePermissionChoice,
  escapePermissionStage,
  initialPermissionFooterState,
  shiftPermissionChoice,
} from "../src/components/permission-footer-state.js";

describe("permission footer state", () => {
  it("submits allow once from the default choice", () => {
    expect(activatePermissionChoice(initialPermissionFooterState()).reply).toEqual({
      decision: "allow",
    });
  });

  it("requires confirmation before always allow", () => {
    const selectedAlways = shiftPermissionChoice(initialPermissionFooterState(), 1);
    const confirmation = activatePermissionChoice(selectedAlways);
    expect(confirmation.state).toMatchObject({ stage: "always", selected: 0 });
    expect(activatePermissionChoice(confirmation.state).reply).toEqual({
      decision: "always_allow",
    });
  });

  it("collects trimmed rejection feedback and supports escape", () => {
    const reject = escapePermissionStage(initialPermissionFooterState());
    expect(reject).toMatchObject({ stage: "reject", selected: 2 });
    expect(activatePermissionChoice({ ...reject, feedback: "  unsafe  " }).reply).toEqual({
      decision: "deny",
      reason: "unsafe",
    });
    expect(escapePermissionStage(reject)).toMatchObject({ stage: "permission", selected: 2 });
  });

  it("wraps option navigation", () => {
    expect(shiftPermissionChoice(initialPermissionFooterState(), -1).selected).toBe(2);
    expect(
      shiftPermissionChoice({ stage: "always", selected: 1, feedback: "" }, 1).selected,
    ).toBe(0);
  });
});
