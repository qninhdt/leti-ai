// /plan slash command — submits a synthetic EnterPlanMode tool call so
// the model treats the next turn as a plan-mode entry. F8 strict: this
// only ENTERS plan mode; exit is the model's job via ExitPlanMode.

import type { Command, CommandContext } from "../registry.js";

export const planCommand: Command = {
  name: "plan",
  category: "Tools",
  summary: "Enter plan mode (read-only profile; model exits via ExitPlanMode)",
  run: async (ctx: CommandContext) => {
    await ctx.enterPlanMode();
  },
};
