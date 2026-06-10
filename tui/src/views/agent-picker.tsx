import React from "react";
import { Box, Text } from "ink";

import { theme } from "../theme/index.js";
import { useListNavigation } from "../hooks/use-list-navigation.js";

import type { AgentDto } from "../api/types.js";

export interface AgentPickerProps {
  agents: AgentDto[];
  onSelect: (id: string) => void;
  onCancel: () => void;
}

export function AgentPicker(props: AgentPickerProps): React.ReactElement {
  const { index } = useListNavigation(
    props.agents,
    (agent) => props.onSelect(agent.id),
    props.onCancel,
  );

  return (
    <Box flexDirection="column" borderStyle="round" borderColor={theme.border.active} paddingX={1}>
      <Text bold color={theme.tool.name}>Agents</Text>
      {props.agents.length === 0 && <Text color={theme.text.muted}>(no agents registered)</Text>}
      {props.agents.map((a, i) => (
        <Box key={a.id}>
          <Text color={i === index ? theme.permission.selected : theme.text.primary}>
            {i === index ? "▸ " : "  "}{a.display_name}
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
