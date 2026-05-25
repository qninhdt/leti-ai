import React from "react";
import { Box, Text } from "ink";

import { theme } from "../theme/index.js";
import { MarkdownRenderer } from "./markdown-renderer.js";
import { ToolCallCard } from "./tool-call-card.js";
import { formatUsd } from "../utils/format.js";

import type { MessageView, PartView } from "../store/index.js";

export interface MessageListProps {
  messages: MessageView[];
  planMode?: boolean;
}

export function MessageList(props: MessageListProps): React.ReactElement {
  return (
    <Box flexDirection="column">
      {props.planMode && (
        <Box marginBottom={1}>
          <Text color={theme.badge.accent} bold>
            ▣ Plan mode
          </Text>
          <Text color={theme.text.muted}> · read-only profile until ExitPlanMode</Text>
        </Box>
      )}
      {props.messages.map((m) => (
        <MessageCard key={m.id} message={m} />
      ))}
    </Box>
  );
}

function MessageCard({ message }: { message: MessageView }): React.ReactElement {
  return (
    <Box flexDirection="column" marginBottom={1}>
      <Box>
        <Text color={roleColor(message.role)} bold>
          {roleLabel(message.role)}
        </Text>
      </Box>
      {message.parts.map((p) => (
        <PartCard key={p.id} part={p} />
      ))}
      {message.step_finish && (
        <Box>
          <Text color={theme.text.muted}>
            ─ {message.step_finish.reason}
            {message.step_finish.usage_total !== undefined ? ` · ${message.step_finish.usage_total} tok` : ""}
            {message.step_finish.cost ? ` · ${formatUsd(message.step_finish.cost)}` : ""}
          </Text>
        </Box>
      )}
    </Box>
  );
}

function PartCard({ part }: { part: PartView }): React.ReactElement {
  const finalized = part.text ?? "";
  const tail = part.buffer;
  switch (part.kind) {
    case "text":
      return (
        <Box flexDirection="column">
          <MarkdownRenderer source={finalized} />
          {tail && <Text color={theme.text.primary}>{tail}</Text>}
        </Box>
      );
    case "reasoning":
      return (
        <Box>
          <Text color={theme.text.muted}>▶ Thinking{part.status === "complete" ? " (done)" : "…"}</Text>
        </Box>
      );
    case "tool_call":
      return (
        <ToolCallCard
          name={part.tool_name ?? "tool"}
          detail={summarizeArgs(part.tool_args)}
          status={part.status}
        />
      );
    case "tool_result":
      return (
        <Box>
          <Text color={theme.tool.ok}>✓ </Text>
          <Text color={theme.text.primary}>
            {summarizeResult(part.tool_result)}
          </Text>
        </Box>
      );
    case "step_finish":
      return <Text color={theme.text.muted}>{part.reason ?? ""}</Text>;
    default:
      return <Text>{finalized}</Text>;
  }
}

function roleLabel(role: string): string {
  switch (role) {
    case "user": return "you";
    case "assistant": return "openlet";
    case "tool": return "tool";
    case "system": return "system";
    default: return role;
  }
}

function roleColor(role: string): string {
  switch (role) {
    case "user": return theme.badge.accent;
    case "assistant": return theme.tool.name;
    case "tool": return theme.text.muted;
    default: return theme.text.muted;
  }
}

function summarizeArgs(args: unknown): string {
  if (!args) return "";
  try {
    const s = JSON.stringify(args);
    return s.length > 80 ? s.slice(0, 77) + "…" : s;
  } catch {
    return "";
  }
}

function summarizeResult(result: unknown): string {
  if (!result) return "(no output)";
  if (typeof result === "string") {
    return result.length > 200 ? result.slice(0, 197) + "…" : result;
  }
  return summarizeArgs(result);
}
