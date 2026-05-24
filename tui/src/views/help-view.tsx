import React from "react";
import { Box, Text } from "ink";

import { commands } from "../commands/registry.js";
import { theme } from "../theme/index.js";

export function HelpView(): React.ReactElement {
  const byCategory = group(commands, (c) => c.category);
  return (
    <Box flexDirection="column" borderStyle="round" borderColor={theme.border.muted} paddingX={1}>
      <Text bold color={theme.tool.name}>Slash commands</Text>
      {(Object.keys(byCategory) as Array<keyof typeof byCategory>).map((cat) => (
        <Box key={cat} flexDirection="column" marginTop={1}>
          <Text bold color={theme.text.heading[2] ?? theme.text.primary}>{cat}</Text>
          {byCategory[cat]!.map((c) => (
            <Box key={c.name}>
              <Text color={theme.permission.selected}>  /{c.name}</Text>
              <Text color={theme.text.muted}> — {c.summary}</Text>
            </Box>
          ))}
        </Box>
      ))}
      <Box marginTop={1} flexDirection="column">
        <Text bold color={theme.text.heading[2] ?? theme.text.primary}>Keyboard</Text>
        <Text color={theme.text.muted}>  Up/Down       History</Text>
        <Text color={theme.text.muted}>  Tab           Complete commands</Text>
        <Text color={theme.text.muted}>  Shift+Enter   Newline</Text>
        <Text color={theme.text.muted}>  Ctrl+C        Cancel/exit</Text>
      </Box>
      <Box marginTop={1}>
        <Text color={theme.text.muted}>Esc to dismiss.</Text>
      </Box>
    </Box>
  );
}

function group<T, K extends string>(items: T[], key: (t: T) => K): Record<K, T[]> {
  const out: Record<string, T[]> = {};
  for (const it of items) {
    const k = key(it);
    if (!out[k]) out[k] = [];
    out[k]!.push(it);
  }
  return out as Record<K, T[]>;
}
