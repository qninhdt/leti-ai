// Derived read-models for the prompt editor. Folds the active session, its
// agent, and the latest step_finish readouts (usage / cost / context tokens)
// out of the store selectors into the memos the layout + meta row consume.
// contextTokens tracks the compaction anchor so the context bar and
// should_compact see the same number.

import { createMemo, type Accessor } from "solid-js";

import { formatTokens, formatUsd } from "../utils/format.js";

import type { AgentDto, SessionDto } from "../api/types.js";
import type { MessageView } from "../store/index.js";

export interface PromptDerivedDeps {
  activeSessionId: Accessor<string | null>;
  sessions: Accessor<Record<string, SessionDto>>;
  agents: Accessor<AgentDto[]>;
  messages: Accessor<MessageView[]>;
  oc: { border: string; borderActive: string };
}

export interface PromptDerived {
  session: Accessor<SessionDto | null>;
  agent: Accessor<AgentDto | null>;
  model: Accessor<string>;
  accent: Accessor<string>;
  streaming: Accessor<boolean>;
  usageText: Accessor<string | undefined>;
  costText: Accessor<string | undefined>;
  contextTokens: Accessor<number | undefined>;
}

export function createPromptDerived(deps: PromptDerivedDeps): PromptDerived {
  const { activeSessionId, sessions, agents, messages, oc } = deps;

  const session = createMemo(() => {
    const id = activeSessionId();
    return id ? sessions()[id] ?? null : null;
  });
  const agent = createMemo(() => {
    const s = session();
    const list = agents();
    return s ? list.find((a) => a.id === s.agent_id) ?? null : list[0] ?? null;
  });
  const model = createMemo(() => agent()?.model ?? "—");
  const accent = createMemo(() => (agent() ? oc.borderActive : oc.border));
  const streaming = createMemo(() => session()?.status === "running");

  const usageText = createMemo(() => {
    const list = messages();
    for (let i = list.length - 1; i >= 0; i--) {
      const total = list[i]?.step_finish?.usage_total;
      if (total && total > 0) return formatTokens(total);
    }
    return undefined;
  });
  const costText = createMemo(() => {
    const raw = session()?.cost_decimal_str;
    if (!raw) return undefined;
    return Number.parseFloat(raw) > 0 ? formatUsd(raw) : undefined;
  });

  // Latest provider-reported prompt tokens — the compaction anchor, so the
  // context bar tracks the same number should_compact does. Undefined before
  // the first turn returns usage (bar degrades to window size only).
  const contextTokens = createMemo(() => {
    const list = messages();
    for (let i = list.length - 1; i >= 0; i--) {
      const tok = list[i]?.step_finish?.context_tokens;
      if (tok && tok > 0) return tok;
    }
    return undefined;
  });

  return { session, agent, model, accent, streaming, usageText, costText, contextTokens };
}
