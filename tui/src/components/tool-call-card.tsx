import React from "react";
import { Box, Text } from "ink";

import { theme } from "../theme/index.js";

export interface ToolCallCardProps {
  name: string;
  detail?: string;
  status?: "streaming" | "complete" | "errored";
}

// ╭─ NAME ─╮ shape.
// `theme.border.muted` ≈ ANSI 245; tool name in bold cyan.
export function ToolCallCard(props: ToolCallCardProps): React.ReactElement {
  return (
    <Box flexDirection="column" marginY={0}>
      <Box>
        <Text color={theme.border.muted}>╭─ </Text>
        <Text color={theme.tool.name} bold>{props.name}</Text>
        <Text color={theme.border.muted}> ─╮</Text>
      </Box>
      {props.detail && (
        <Box>
          <Text color={theme.border.muted}>│ </Text>
          <Text color={theme.text.primary}>{props.detail}</Text>
        </Box>
      )}
      <Box>
        <Text color={theme.border.muted}>╰─{props.status === "errored" ? " errored" : ""}</Text>
      </Box>
    </Box>
  );
}
