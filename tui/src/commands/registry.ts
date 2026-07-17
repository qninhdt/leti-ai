// Slash command registry — single source of truth for /help.
// `/danger` toggles permission mode; `/plugins` lists installed plugins.

import { planCommand } from "./builtins/plan.js";

export type Suggestion = {
  display: string;
  description: string;
  insert: string;
};

export interface CommandContext {
  setView: (view: { kind: string; askId?: string }) => void;
  cancelTurn: () => Promise<void>;
  newSession: () => Promise<void>;
  compact: () => Promise<void>;
  setMode: (mode: "read_only" | "workspace_write" | "danger") => Promise<void>;
  enterPlanMode: () => Promise<void>;
  exit: () => void;
}

export interface Command {
  name: string;
  aliases?: string[];
  category: "Session" | "Tools" | "Config" | "Debug";
  summary: string;
  run: (ctx: CommandContext) => void | Promise<void>;
}

export const commands: Command[] = [
  {
    name: "help",
    category: "Debug",
    summary: "Show available commands",
    run: (ctx) => ctx.setView({ kind: "help" }),
  },
  {
    name: "agents",
    category: "Session",
    summary: "Open agent picker",
    run: (ctx) => ctx.setView({ kind: "agent_picker" }),
  },
  {
    name: "sessions",
    aliases: ["resume"],
    category: "Session",
    summary: "List recent sessions",
    run: (ctx) => ctx.setView({ kind: "session_picker" }),
  },
  {
    name: "new",
    category: "Session",
    summary: "Start a new session",
    run: (ctx) => ctx.newSession(),
  },
  {
    name: "clear",
    category: "Session",
    summary: "Clear conversation (start a fresh session)",
    run: (ctx) => ctx.newSession(),
  },
  {
    name: "cancel",
    category: "Session",
    summary: "Abort current turn",
    run: (ctx) => ctx.cancelTurn(),
  },
  {
    name: "compact",
    category: "Session",
    summary: "Summarize older messages to free up context",
    run: (ctx) => ctx.compact(),
  },
  {
    name: "danger",
    category: "Config",
    summary: "Toggle DANGER permission mode (skips all asks)",
    run: (ctx) => ctx.setMode("danger"),
  },
  {
    name: "plugins",
    category: "Debug",
    summary: "List installed plugins",
    run: (ctx) => ctx.setView({ kind: "plugins" }),
  },
  planCommand,
  {
    name: "quit",
    aliases: ["exit", "q"],
    category: "Session",
    summary: "Quit leti TUI",
    run: (ctx) => ctx.exit(),
  },
];

const commandByToken = buildCommandLookup(commands);

export function findCommand(query: string): Command | undefined {
  return commandByToken.get(normalizeCommandQuery(query));
}

export function complete(query: string): Suggestion[] {
  const q = normalizeCommandQuery(query);
  if (!q) return commands.map(asSuggestion);
  return commands
    .filter((c) => c.name.startsWith(q) || c.aliases?.some((a) => a.startsWith(q)))
    .map(asSuggestion);
}

function normalizeCommandQuery(query: string): string {
  return query.trim().toLowerCase().replace(/^\//, "");
}

function buildCommandLookup(source: Command[]): Map<string, Command> {
  const lookup = new Map<string, Command>();

  for (const command of source) {
    for (const token of [command.name, ...(command.aliases ?? [])]) {
      const normalized = normalizeCommandQuery(token);
      const existing = lookup.get(normalized);

      if (existing) {
        throw new Error(`Duplicate slash command token "${normalized}" for /${existing.name} and /${command.name}`);
      }

      lookup.set(normalized, command);
    }
  }

  return lookup;
}

function asSuggestion(c: Command): Suggestion {
  return { display: `/${c.name}`, description: c.summary, insert: `/${c.name} ` };
}
