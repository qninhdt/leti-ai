import React from "react";
import { Box, useApp } from "ink";

import { ChatView } from "./views/chat-view.js";
import { AgentPicker } from "./views/agent-picker.js";
import { SessionPicker } from "./views/session-picker.js";
import { PermissionModal } from "./views/permission-modal.js";
import { HelpView } from "./views/help-view.js";
import { PluginsView } from "./views/plugins-view.js";
import { complete, findCommand } from "./commands/registry.js";
import { connectSse } from "./api/sse.js";
import { useStore } from "./store/index.js";
import { PromptHistory } from "./hooks/use-prompt-history.js";
import { randomId } from "./utils/id.js";

import type { OpenletClient } from "./api/client.js";
import type { CreateMessageDto } from "./api/types.js";

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
    // Guard: empty/whitespace-only buffer never reaches the server. An
    // empty Enter would otherwise POST an empty-text prompt and burn a
    // turn. Slash-commands are still allowed to be bare (e.g. "/help").
    if (!text.trim()) return;
    try {
      store.setClientError(null);
      await runSubmit(text);
    } catch (err) {
      // submit() is invoked from PromptEditor's synchronous useInput
      // handler, so a rejected promise here would be unhandled (and can
      // crash the process). Surface it as a banner instead.
      store.setClientError(err instanceof Error ? err.message : String(err));
    }
  };

  const runSubmit = async (text: string) => {
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
          enterPlanMode: async () => {
            // F8 strict: /plan only ENTERS plan mode. Exit is via the
            // model's ExitPlanMode tool. We submit a synthetic user
            // message asking the model to enter plan mode; the model
            // then issues the EnterPlanMode tool call. This keeps the
            // entry path purely in-conversation so the operator's
            // intent is auditable in the message log.
            if (!store.activeSessionId) return;
            await props.client.promptAsync(
              store.activeSessionId,
              textPrompt(
                "Please enter plan mode now using the enter_plan_mode tool, then gather context and produce a plan via exit_plan_mode.",
              ),
            );
          },
          exit,
        });
        return;
      }
    }
    if (!store.activeSessionId) return;
    props.history.push(text);
    setHistoryIdx(null);
    await props.client.promptAsync(store.activeSessionId, textPrompt(text));
  };

  // Bind to a const so TS keeps the discriminated-union narrowing inside
  // the JSX closures below (a `store.view.askId` access would re-widen).
  const view = store.view;
  return (
    <Box flexDirection="column">
      {view.kind === "chat" && (
        <ChatView
          conn={store.conn.status}
          session={store.activeSessionId ? store.sessions[store.activeSessionId] ?? null : null}
          agent={store.agents.find((a) => a.id === activeAgentId(store)) ?? null}
          messages={store.activeSessionId ? store.messages[store.activeSessionId] ?? [] : []}
          pluginErrors={store.pluginErrors}
          clientError={store.clientError}
          planMode={store.activeSessionId ? !!store.planMode[store.activeSessionId] : false}
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
      {view.kind === "agent_picker" && (
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
      {view.kind === "session_picker" && (
        <SessionPicker
          sessions={Object.values(store.sessions)}
          onSelect={(id) => {
            store.setActiveSession(id);
            store.setView({ kind: "chat" });
          }}
          onCancel={() => store.setView({ kind: "chat" })}
        />
      )}
      {view.kind === "permission" && (
        <PermissionModal
          request={store.pendingPermissions[view.askId]!}
          onResolve={async (reply) => {
            await props.client.replyPermission(view.askId, {
              reply: reply.decision,
              pattern: reply.pattern ?? null,
              feedback: reply.feedback ?? null,
            });
            store.setView({ kind: "chat" });
          }}
        />
      )}
      {view.kind === "help" && <HelpView />}
      {view.kind === "plugins" && <PluginsView plugins={store.plugins} />}
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

// Build a single text-part user message. The part id is client-generated
// (see utils/id.ts) and consumed by the server's part validation.
function textPrompt(text: string): CreateMessageDto {
  return {
    parts: [
      {
        id: randomId(),
        message_id: "",
        kind: "text",
        text,
      },
    ],
  };
}
