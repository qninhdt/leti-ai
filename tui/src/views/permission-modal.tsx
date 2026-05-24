import React from "react";
import { Box, Text, useInput } from "ink";
import { createPatch } from "diff";

import { theme } from "../theme/index.js";

import type { PermissionRequestDto } from "../api/types.js";

export interface PermissionModalProps {
  request: PermissionRequestDto;
  onResolve: (reply: {
    decision: "allow" | "deny" | "always";
    pattern?: string;
    feedback?: string;
  }) => void;
}

type Stage = "initial" | "always" | "deny_feedback";

export function PermissionModal(props: PermissionModalProps): React.ReactElement {
  const [stage, setStage] = React.useState<Stage>("initial");
  const [pattern, setPattern] = React.useState(
    props.request.patterns?.[0] ?? props.request.tool_name,
  );
  const [feedback, setFeedback] = React.useState("");

  useInput((input, key) => {
    if (stage === "initial") {
      if (input === "1" || input === "a") props.onResolve({ decision: "allow" });
      else if (input === "2" || input === "A") setStage("always");
      else if (input === "3" || input === "r" || key.escape)
        props.onResolve({ decision: "deny" });
    } else if (stage === "always") {
      if (key.escape) setStage("initial");
      else if (key.return) props.onResolve({ decision: "always", pattern });
      else if (key.backspace || key.delete) setPattern((p) => p.slice(0, -1));
      else if (input) setPattern((p) => p + input);
    } else if (stage === "deny_feedback") {
      if (key.escape) setStage("initial");
      else if (key.return)
        props.onResolve({ decision: "deny", feedback: feedback || undefined });
      else if (key.backspace || key.delete) setFeedback((f) => f.slice(0, -1));
      else if (input) setFeedback((f) => f + input);
    }
  });

  return (
    <Box flexDirection="column" borderStyle="double" borderColor={theme.permission.border} paddingX={1}>
      <Text color={theme.permission.title} bold>
        Permission required: {props.request.permission}
      </Text>
      <PermissionBody request={props.request} />
      {stage === "initial" && (
        <Box flexDirection="column" marginTop={1}>
          <Text>
            <Text color={theme.permission.selected}>[1/a]</Text> allow once   {" "}
            <Text color={theme.permission.selected}>[2/A]</Text> always   {" "}
            <Text color={theme.permission.danger}>[3/r/Esc]</Text> deny
          </Text>
        </Box>
      )}
      {stage === "always" && (
        <Box flexDirection="column" marginTop={1}>
          <Text color={theme.text.muted}>Edit pattern (Enter to confirm, Esc to back):</Text>
          <Text color={theme.text.primary}>{pattern}<Text color={theme.border.active}>▌</Text></Text>
        </Box>
      )}
      {stage === "deny_feedback" && (
        <Box flexDirection="column" marginTop={1}>
          <Text color={theme.text.muted}>Feedback for the agent (Enter to confirm, Esc to back):</Text>
          <Text color={theme.text.primary}>{feedback}<Text color={theme.border.active}>▌</Text></Text>
        </Box>
      )}
    </Box>
  );
}

function PermissionBody({ request }: { request: PermissionRequestDto }): React.ReactElement {
  if (request.diff) {
    const patch = createPatch(
      request.diff.filepath,
      request.diff.before,
      request.diff.after,
      "",
      "",
    );
    return (
      <Box flexDirection="column" marginTop={1}>
        <Text color={theme.text.muted}>{request.diff.filepath}</Text>
        {patch.split("\n").slice(4).map((line, i) => (
          <Text key={i} color={diffColor(line)}>{line}</Text>
        ))}
      </Box>
    );
  }
  if (request.bash) {
    return (
      <Box flexDirection="column" marginTop={1}>
        <Box>
          <Text color={theme.text.muted}>$ </Text>
          <Text color={theme.text.primary}>{request.bash.command}</Text>
        </Box>
        <Text color={theme.text.muted}>
          cwd: {request.bash.cwd} · timeout: {request.bash.timeout_ms}ms
        </Text>
      </Box>
    );
  }
  return (
    <Box marginTop={1}>
      <Text color={theme.text.muted}>{request.tool_name}</Text>
      {request.reason && <Text color={theme.text.muted}> — {request.reason}</Text>}
    </Box>
  );
}

function diffColor(line: string): string {
  if (line.startsWith("+") && !line.startsWith("+++")) return theme.diff.added;
  if (line.startsWith("-") && !line.startsWith("---")) return theme.diff.removed;
  if (line.startsWith("@@")) return theme.border.muted;
  return theme.text.primary;
}
