import React from "react";
import { Box, Text, useInput } from "ink";

import { theme } from "../theme/index.js";

import type { AgentDto } from "../api/types.js";

export interface AgentPickerProps {
  agents: AgentDto[];
  onSelect: (id: string) => void;
  onCancel: () => void;
}

export function AgentPicker(props: AgentPickerProps): React.ReactElement {
  const [index, setIndex] = React.useState(0);

  useInput((_input, key) => {
    if (key.escape) return props.onCancel();
    if (key.upArrow) setIndex((i) => Math.max(0, i - 1));
    if (key.downArrow) setIndex((i) => Math.min(props.agents.length - 1, i + 1));
    if (key.return) {
      const choice = props.agents[index];
      if (choice) props.onSelect(choice.id);
    }
  });

  return (
    <Box flexDirection="column" borderStyle="round" borderColor={theme.border.active} paddingX={1}>
      <Text bold color={theme.tool.name}>Agents</Text>
      {props.agents.length === 0 && <Text color={theme.text.muted}>(no agents registered)</Text>}
      {props.agents.map((a, i) => (
        <Box key={a.id}>
          <Text color={i === index ? theme.permission.selected : theme.text.primary}>
            {i === index ? "▸ " : "  "}{a.name}
          </Text>
          {a.description && (
            <Text color={theme.text.muted}> — {a.description}</Text>
          )}
        </Box>
      ))}
      <Text color={theme.text.muted}>↑↓ select · Enter confirm · Esc cancel</Text>
    </Box>
  );
}
