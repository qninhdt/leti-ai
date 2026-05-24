import React from "react";
import { Box, Text } from "ink";

import { theme } from "../theme/index.js";

import type { PluginInfoDto } from "../api/types.js";

export function PluginsView({ plugins }: { plugins: PluginInfoDto[] }): React.ReactElement {
  return (
    <Box flexDirection="column" borderStyle="round" borderColor={theme.border.muted} paddingX={1}>
      <Text bold color={theme.tool.name}>Plugins</Text>
      {plugins.length === 0 && <Text color={theme.text.muted}>(none reported)</Text>}
      {plugins.map((p) => (
        <Box key={p.id} flexDirection="column">
          <Box>
            <Text color={p.enabled ? theme.badge.success : theme.text.muted}>
              {p.enabled ? "● " : "○ "}
            </Text>
            <Text color={theme.text.primary}>{p.name}</Text>
            <Text color={theme.text.muted}> v{p.version}</Text>
          </Box>
          <Text color={theme.text.muted}>  capabilities: {p.capabilities.join(", ") || "—"}</Text>
        </Box>
      ))}
      <Text color={theme.text.muted}>Esc to dismiss.</Text>
    </Box>
  );
}
