import React from "react";
import { Box, Text } from "ink";

import { theme } from "../theme/index.js";
import { formatUsd, shortId } from "../utils/format.js";

import type { ConnState } from "../api/sse.js";

export interface StatusBarProps {
  conn: ConnState;
  sessionId: string | null;
  agentName?: string;
  modelName?: string;
  cost?: string;
  permissionMode?: string;
}

const CONN_GLYPH: Record<ConnState, string> = {
  idle: "○",
  connecting: "◔",
  open: "●",
  reconnecting: "◐",
  error: "●",
};

const CONN_COLOR: Record<ConnState, string> = {
  idle: theme.text.muted,
  connecting: theme.badge.accent,
  open: theme.badge.success,
  reconnecting: theme.badge.warning,
  error: theme.badge.error,
};

export function StatusBar(props: StatusBarProps): React.ReactElement {
  const sep = (
    <Text color={theme.border.muted}> │ </Text>
  );
  return (
    <Box paddingX={1} borderStyle="single" borderColor={theme.border.muted}>
      <Text color={CONN_COLOR[props.conn]}>{CONN_GLYPH[props.conn]} openlet</Text>
      {sep}
      <Text color={theme.text.muted}>session:</Text>
      <Text color={theme.text.primary}>{props.sessionId ? shortId(props.sessionId) : "—"}</Text>
      {sep}
      <Text color={theme.text.muted}>agent:</Text>
      <Text color={theme.tool.name}>{props.agentName ?? "—"}</Text>
      {sep}
      <Text color={theme.text.muted}>model:</Text>
      <Text color={theme.text.primary}>{props.modelName ?? "—"}</Text>
      {sep}
      <Text color={theme.badge.warning}>{formatUsd(props.cost)}</Text>
      {props.permissionMode === "danger" && (
        <>
          {sep}
          <Text color={theme.permission.danger} bold>DANGER</Text>
        </>
      )}
      {props.conn === "reconnecting" && (
        <>
          {sep}
          <Text color={theme.badge.warning}>reconnecting…</Text>
        </>
      )}
    </Box>
  );
}
