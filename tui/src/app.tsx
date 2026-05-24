import React from "react";
import { Box, Text, useApp } from "ink";

import { ChatView } from "./views/chat-view.js";
import { AgentPicker } from "./views/agent-picker.js";
import { SessionPicker } from "./views/session-picker.js";
import { PermissionModal } from "./views/permission-modal.js";
import { HelpView } from "./views/help-view.js";
import { PluginsView } from "./views/plugins-view.js";
import { commands, complete, findCommand } from "./commands/registry.js";
import { connectSse } from "./api/sse.js";
import { useStore } from "./store/index.js";
import { theme } from "./theme/index.js";
import { PromptHistory } from "./hooks/use-prompt-history.js";

import type { OpenletClient } from "./api/client.js";

export interface AppProps {
  client: OpenletClient;
  baseUrl: string;
  history: PromptHistory;
  token?: string;
}

export function App(props: AppProps): React.ReactElement {
  const store = useStore();
  const { exit } = useApp();
  const [prompt, setPrompt] = React.useState("");
  const [historyIdx, setHistoryIdx] = React.useState<number | null>(null);

  React.useEffect(() => {
    void bootstrap(props.client, store);
    const sse = connectSse({
      baseUrl: props.baseUrl,
      sessionId: store.activeSessionId ?? undefined,
      token: props.token,
      onEvent: store.applyEvent,
      onState: (status, detail) => store.setConn(status, detail),
    });
    return () => sse.close();
  }, [props.baseUrl, props.client, props.token, store.activeSessionId]);

  const submit = async (text: string) => {
    if (text.startsWith("/")) {
      const [name] = text.slice(1).split(/\s+/);
      const cmd = findCommand(name ?? "");
      if (cmd) {
        await cmd.run({
          setView: (v) => store.setView(v as never),
          cancelTurn: async () => {
            if (store.activeSessionId) await props.client.abort(store.activeSessionId);
          },
          newSession: async () => {
            const agent = store.agents[0];
            if (!agent) return;
            const s = await props.client.createSession({ agent_id: agent.id });
            store.setSessions([...Object.values(store.sessions), s]);
            store.setActiveSession(s.id);
          },
          setMode: async (mode) => {
            if (store.activeSessionId) await props.client.setMode(store.activeSessionId, { mode });
          },
          exit,
        });
        return;
      }
    }
    if (!store.activeSessionId) return;
    props.history.push(text);
    setHistoryIdx(null);
    await props.client.promptAsync(store.activeSessionId, { text });
  };

  return (
    <Box flexDirection="column">
      {store.view.kind === "chat" && (
        <ChatView
          conn={store.conn.status}
          session={store.activeSessionId ? store.sessions[store.activeSessionId] ?? null : null}
          agent={store.agents.find((a) => a.id === activeAgentId(store)) ?? null}
          messages={store.activeSessionId ? store.messages[store.activeSessionId] ?? [] : []}
          pluginErrors={store.pluginErrors}
          promptValue={prompt}
          setPromptValue={setPrompt}
          onSubmit={submit}
          onHistoryUp={() => navigateHistory(props.history, historyIdx, -1, setHistoryIdx, setPrompt)}
          onHistoryDown={() => navigateHistory(props.history, historyIdx, 1, setHistoryIdx, setPrompt)}
          onTabComplete={() => {
            const s = complete(prompt);
            if (s.length === 1) setPrompt(s[0]!.insert);
          }}
        />
      )}
      {store.view.kind === "agent_picker" && (
        <AgentPicker
          agents={store.agents}
          onSelect={async (id) => {
            const session = await props.client.createSession({ agent_id: id });
            store.setSessions([...Object.values(store.sessions), session]);
            store.setActiveSession(session.id);
            store.setView({ kind: "chat" });
          }}
          onCancel={() => store.setView({ kind: "chat" })}
        />
      )}
      {store.view.kind === "session_picker" && (
        <SessionPicker
          sessions={Object.values(store.sessions)}
          onSelect={(id) => {
            store.setActiveSession(id);
            store.setView({ kind: "chat" });
          }}
          onCancel={() => store.setView({ kind: "chat" })}
        />
      )}
      {store.view.kind === "permission" && (
        <PermissionModal
          request={store.pendingPermissions[store.view.askId]!}
          onResolve={async (reply) => {
            await props.client.replyPermission(store.view.kind === "permission" ? store.view.askId : "", {
              reply: reply.decision,
              pattern: reply.pattern ?? null,
              feedback: reply.feedback ?? null,
            });
            store.setView({ kind: "chat" });
          }}
        />
      )}
      {store.view.kind === "help" && <HelpView />}
      {store.view.kind === "plugins" && <PluginsView plugins={store.plugins} />}
      {commands.length === 0 && (
        <Text color={theme.text.muted}>(no commands registered)</Text>
      )}
    </Box>
  );
}

async function bootstrap(client: OpenletClient, store: ReturnType<typeof useStore.getState>): Promise<void> {
  const [agents, plugins, sessions] = await Promise.all([
    client.listAgents().catch(() => []),
    client.listPlugins().catch(() => []),
    client.listSessions().catch(() => []),
  ]);
  store.setAgents(agents);
  store.setPlugins(plugins);
  store.setSessions(sessions);
}

function activeAgentId(store: ReturnType<typeof useStore.getState>): string | null {
  if (!store.activeSessionId) return null;
  return store.sessions[store.activeSessionId]?.agent_id ?? null;
}

function navigateHistory(
  history: PromptHistory,
  current: number | null,
  delta: -1 | 1,
  setIdx: (n: number | null) => void,
  setPrompt: (s: string) => void,
): void {
  const list = history.list();
  if (list.length === 0) return;
  const next = current === null ? list.length - 1 : current + delta;
  if (next < 0 || next >= list.length) {
    setIdx(null);
    if (delta > 0) setPrompt("");
    return;
  }
  setIdx(next);
  setPrompt(list[next]!);
}
