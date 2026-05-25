import React from "react";
import { Box, Text } from "ink";

import { MessageList } from "../components/message-list.js";
import { PromptEditor } from "../components/prompt-editor.js";
import { StatusBar } from "../components/status-bar.js";
import { theme } from "../theme/index.js";

import type { ConnState } from "../api/sse.js";
import type { AgentDto, SessionDto } from "../api/types.js";
import type { MessageView, PluginErrorView } from "../store/index.js";

export interface ChatViewProps {
  conn: ConnState;
  session: SessionDto | null;
  agent: AgentDto | null;
  messages: MessageView[];
  pluginErrors: PluginErrorView[];
  planMode: boolean;
  promptValue: string;
  setPromptValue: (v: string) => void;
  onSubmit: (v: string) => void;
  onHistoryUp: () => void;
  onHistoryDown: () => void;
  onTabComplete: () => void;
}

export function ChatView(props: ChatViewProps): React.ReactElement {
  return (
    <Box flexDirection="column">
      <MessageList messages={props.messages} planMode={props.planMode} />
      {props.pluginErrors.length > 0 && (
        <PluginErrorBanner err={props.pluginErrors[props.pluginErrors.length - 1]!} />
      )}
      <PromptEditor
        value={props.promptValue}
        onChange={props.setPromptValue}
        onSubmit={props.onSubmit}
        onHistoryUp={props.onHistoryUp}
        onHistoryDown={props.onHistoryDown}
        onTabComplete={props.onTabComplete}
      />
      <StatusBar
        conn={props.conn}
        sessionId={props.session?.id ?? null}
        agentName={props.agent?.name}
        modelName={props.agent?.model ?? undefined}
        cost={props.session?.cost_decimal_str}
        permissionMode={props.session?.permission_mode}
      />
    </Box>
  );
}

function PluginErrorBanner({ err }: { err: PluginErrorView }): React.ReactElement {
  return (
    <Box borderStyle="single" borderColor={theme.permission.danger} paddingX={1}>
      <Text color={theme.permission.danger} bold>plugin.error</Text>
      <Text color={theme.text.muted}> {err.pluginId} </Text>
      <Text color={theme.text.primary}>{err.message}</Text>
    </Box>
  );
}
