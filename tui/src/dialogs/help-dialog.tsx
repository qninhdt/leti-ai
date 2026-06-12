// Help overlay content. Lists slash commands grouped by category plus the
// keyboard reference. Pure content — it registers no overlay key handler, so
// the router's Esc-pops-overlay path closes it.

import { For } from "solid-js";

import { theme } from "../theme/index.js";
import { commands, type Command } from "../commands/registry.js";

const KEYBINDINGS: Array<[string, string]> = [
  ["Up/Down", "History"],
  ["Enter", "Send · Shift+Enter newline"],
  ["Esc", "Interrupt / close overlay"],
  ["⌘K / ctrl+k", "Command palette"],
  ["Ctrl+C", "Quit"],
];

function groupByCategory(list: Command[]): Array<[string, Command[]]> {
  const map = new Map<string, Command[]>();
  for (const c of list) {
    const bucket = map.get(c.category) ?? [];
    bucket.push(c);
    map.set(c.category, bucket);
  }
  return [...map.entries()];
}

export function HelpDialog() {
  const oc = theme.oc;
  const groups = groupByCategory(commands);

  return (
    <box flexDirection="column" minWidth={48}>
      <text fg={oc.primary}>Slash commands</text>
      <For each={groups}>
        {([category, cmds]) => (
          <box flexDirection="column" marginTop={1}>
            <text fg={oc.secondary}>{category}</text>
            <For each={cmds}>
              {(c) => (
                <box flexDirection="row">
                  <text fg={oc.accent}>  /{c.name}</text>
                  <text fg={oc.textMuted}> — {c.summary}</text>
                </box>
              )}
            </For>
          </box>
        )}
      </For>
      <box flexDirection="column" marginTop={1}>
        <text fg={oc.secondary}>Keyboard</text>
        <For each={KEYBINDINGS}>
          {([combo, desc]) => (
            <box flexDirection="row">
              <text fg={oc.text}>  {combo}</text>
              <text fg={oc.textMuted}> — {desc}</text>
            </box>
          )}
        </For>
      </box>
      <text fg={oc.textMuted}>Esc to dismiss.</text>
    </box>
  );
}
