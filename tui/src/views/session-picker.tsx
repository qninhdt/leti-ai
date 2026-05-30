import React from "react";
import { Box, Text } from "ink";

import { theme } from "../theme/index.js";
import { shortId } from "../utils/format.js";
import { useListNavigation } from "../hooks/use-list-navigation.js";

import type { SessionDto } from "../api/types.js";

export interface SessionPickerProps {
  sessions: SessionDto[];
  onSelect: (id: string) => void;
  onCancel: () => void;
}

export function SessionPicker(props: SessionPickerProps): React.ReactElement {
  const list = [...props.sessions].sort((a, b) =>
    b.updated_at.localeCompare(a.updated_at),
  );

  const { index } = useListNavigation(
    list,
    (session) => props.onSelect(session.id),
    props.onCancel,
  );

  return (
    <Box flexDirection="column" borderStyle="round" borderColor={theme.border.active} paddingX={1}>
      <Text bold color={theme.tool.name}>Sessions</Text>
      {list.length === 0 && <Text color={theme.text.muted}>(no sessions yet)</Text>}
      {list.map((s, i) => (
        <Box key={s.id}>
          <Text color={i === index ? theme.permission.selected : theme.text.primary}>
            {i === index ? "▸ " : "  "}
            {shortId(s.id)} · {s.status} · {s.permission_mode}
          </Text>
        </Box>
      ))}
      <Text color={theme.text.muted}>↑↓ select · Enter resume · Esc cancel</Text>
    </Box>
  );
}
