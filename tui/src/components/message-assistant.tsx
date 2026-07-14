// Assistant message, ported from OpenCode's assistant branch
// (`routes/session/index.tsx:1402`). Iterates the message's parts through a
// kind-keyed dispatch (text / reasoning / tool) and renders a per-turn footer
// line `▣ model · cost` once `step_finish` arrives. NO duration — the
// step_finished DTO carries reason/usage/cost only, no timing — so cost is the
// trailing metric. Permission mode is deliberately NOT shown here: it's a
// session-wide setting (default workspace_write), not a per-message fact, and
// repeating it on every turn was noise. It lives in the sidebar instead. A
// tool_result part is folded into its tool_call's block by matching
// tool_call_id, never rendered as a standalone row.
//
// Streaming reconciliation is why the parts loop is <Index> + <Switch>, not
// <For> + a plain switch. The store hands us a NEW parts array (and new part
// objects) on every streamed token. <For> keys by reference, so it would
// dispose and rebuild each part's component — and the markdown renderable it
// owns — on every token, which reads as flicker/lag. <Index> keys by position:
// the streaming text part keeps ONE PartText instance whose `content` prop
// updates in place, so the markdown renderable's incremental parser survives.
// <Switch>/<Match> on part().kind keeps the dispatch reactive, since a part at
// a given position can change kind (a streaming text part is replaced by the
// hydrated, correctly-typed part on the next GET /messages).

import { Index, Show, Switch, Match, createMemo } from "solid-js";

import { theme } from "../theme/index.js";
import { formatUsd } from "../utils/format.js";
import { toolVisual } from "./tool-visuals.js";
import { toolLabel, toolBlockTitle } from "./tool-label.js";
import { formatToolOutput } from "./tool-output-format.js";
import { PartText } from "./part-text.js";
import { PartReasoning } from "./part-reasoning.js";
import { ToolInline } from "./tool-inline.js";
import { ToolBlock } from "./tool-block.js";
import { ToolDiff } from "./tool-diff.js";
import { ToolSubagentBlock } from "./tool-subagent-block.js";
import { parseSubagentCall } from "./tool-subagent-parse.js";
import { useStoreSelector } from "../render/store-bridge.js";
import { parseFileDiff } from "./tool-diff-parse.js";
import { ToolTodo } from "./tool-todo.js";
import { ToolAskUser } from "./tool-ask-user.js";
import { CompactionDivider } from "./compaction-divider.js";

import type { Accessor } from "solid-js";
import type { MessageView, PartView, SubagentView } from "../store/index.js";

export interface MessageAssistantProps {
  message: MessageView;
  /// Agent accent for the ▣ footer glyph.
  accent: string;
  /// Model label for the footer (agent's model, or a dash).
  model: string;
}

function summarize(value: unknown, max = 80): string {
  if (value === undefined || value === null) return "";
  if (typeof value === "string") return value.length > max ? `${value.slice(0, max - 1)}…` : value;
  try {
    const s = JSON.stringify(value);
    return s.length > max ? `${s.slice(0, max - 1)}…` : s;
  } catch {
    return "";
  }
}

function resultText(value: unknown): string {
  if (value === undefined || value === null) return "";
  return typeof value === "string" ? value : summarize(value, 4000);
}

export function MessageAssistant(props: MessageAssistantProps) {
  const oc = theme.oc;
  // Live subagent rows keyed by task_id (fed by subagent.* SSE frames). The
  // task block reads its row via the call's task_id (from the tool result).
  const subagents = useStoreSelector((s) => s.subagents);
  const subagentFor = (p: PartView): SubagentView | undefined => {
    const call = parseSubagentCall(p.tool_args, p.tool_result);
    return call?.taskId ? subagents()[call.taskId] : undefined;
  };

  // Tool-call ids that render as BLOCK cards — those fold their own result, so
  // the standalone result row is suppressed for them. Collect both `id` and
  // `tool_call_id` because a call part may self-identify via either field.
  const blockCallIds = createMemo(() => {
    const ids = new Set<string>();
    for (const p of props.message.parts) {
      if (p.kind === "tool_call" && toolVisual(p.tool_name).template === "block") {
        if (p.tool_call_id) ids.add(p.tool_call_id);
        if (p.id) ids.add(p.id);
      }
    }
    return ids;
  });

  // Result content keyed by the result's tool_call_id, so a block card can look
  // up its output. The matching call finds its result via the shared id.
  const resultByCallId = createMemo(() => {
    const map = new Map<string, unknown>();
    for (const p of props.message.parts) {
      if (p.kind === "tool_result" && p.tool_call_id) map.set(p.tool_call_id, p.tool_result);
    }
    return map;
  });

  // Tool name keyed by call id. A tool_result part carries only tool_call_id
  // (no name), so the inline result row looks the name up here to format the
  // structured JSON body into human text instead of showing a raw blob.
  const toolNameByCallId = createMemo(() => {
    const map = new Map<string, string | undefined>();
    for (const p of props.message.parts) {
      if (p.kind === "tool_call" && p.tool_call_id) map.set(p.tool_call_id, p.tool_name);
    }
    return map;
  });

  // Call ids whose standalone result row must be suppressed because a
  // special renderer already owns the call's display: block cards fold their
  // own result (blockCallIds), and the todo checklist renders from args so its
  // `{count}` result is redundant noise.
  const suppressedResultIds = createMemo(() => {
    const ids = new Set(blockCallIds());
    for (const p of props.message.parts) {
      if (p.kind !== "tool_call") continue;
      const name = (p.tool_name ?? "").toLowerCase();
      // todo + ask_user own their own renderers that already fold the result.
      if (name === "todo" || name === "ask_user") {
        if (p.tool_call_id) ids.add(p.tool_call_id);
        if (p.id) ids.add(p.id);
      }
    }
    return ids;
  });

  // Result body a tool_call part should display: prefer the folded result
  // matched by call id, falling back to any result carried on the part itself.
  const outFor = (p: PartView): unknown => {
    const byId = resultByCallId();
    return (
      (p.tool_call_id ? byId.get(p.tool_call_id) : undefined) ??
      (p.id ? byId.get(p.id) : undefined) ??
      p.tool_result
    );
  };

  // A tool_call part's dispatch (todo / ask_user / block-diff / block / inline),
  // kept reactive so a result folded in by a later hydration flips the branch
  // (e.g. block card → diff card) without recreating the whole message subtree.
  const ToolCallPart = (p: Accessor<PartView>) => {
    const name = () => (p().tool_name ?? "").toLowerCase();
    const out = () => outFor(p());
    const diff = () => parseFileDiff(out());
    const title = () => `# ${toolBlockTitle(p().tool_name, p().tool_args)}`;
    return (
      <Switch
        fallback={
          <ToolInline
            part={p()}
            summary={`${p().tool_name ?? "tool"} ${toolLabel(p().tool_name, p().tool_args)}`.trim()}
          />
        }
      >
        {/* Todo renders as a checklist from its args, not a generic tool line. */}
        <Match when={name() === "todo"}>
          <ToolTodo part={p()} />
        </Match>
        {/* ask_user renders as a question block (options + chosen answer)
            instead of raw JSON. The live selection UI is a separate overlay. */}
        <Match when={name() === "ask_user"}>
          <ToolAskUser part={p()} result={out()} />
        </Match>
        {/* subagent_task renders an inline task block: agent slug, live
            status/output tail, cost. Live state comes from the `subagents`
            store slice keyed by the call's task_id (fed by subagent.* frames);
            a promoted task shows a "result below" affordance instead of the
            output (delivered as an injected parent turn). */}
        <Match when={name() === "subagent_task"}>
          <ToolSubagentBlock part={p()} live={subagentFor(p())} />
        </Match>
        <Match when={toolVisual(p().tool_name).template === "block"}>
          {/* edit/write emit a structured FileDiff in their result body; render
              it as a colored diff card. A write "create" has no diff and falls
              back to the generic block card. */}
          <Show
            when={diff()}
            fallback={
              <ToolBlock
                part={p()}
                title={title()}
                output={formatToolOutput(p().tool_name, resultText(out()))}
              />
            }
          >
            {(d) => <ToolDiff part={p()} title={title()} diff={d()} />}
          </Show>
        </Match>
      </Switch>
    );
  };

  // A standalone tool_result row. Suppressed when a BLOCK tool_call folds it
  // into its card, or when todo/ask_user own the call's display. Inline-tool
  // results, and orphans with no matching call, still render so output is never
  // silently dropped.
  const ToolResultPart = (p: Accessor<PartView>) => {
    const name = () => (p().tool_call_id ? toolNameByCallId().get(p().tool_call_id!) : undefined);
    const summary = () =>
      formatToolOutput(name(), resultText(p().tool_result)).split("\n")[0]?.slice(0, 80) ?? "";
    return (
      <Show when={!p().tool_call_id || !suppressedResultIds().has(p().tool_call_id!)}>
        <ToolInline part={p()} summary={summary()} />
      </Show>
    );
  };

  const finish = () => props.message.step_finish;

  return (
    <box flexDirection="column">
      <Index each={props.message.parts}>
        {(part) => (
          <Switch>
            <Match when={part().kind === "text"}>
              <PartText part={part()} />
            </Match>
            <Match when={part().kind === "reasoning"}>
              <PartReasoning part={part()} />
            </Match>
            <Match when={part().kind === "compaction"}>
              <CompactionDivider part={part()} />
            </Match>
            <Match when={part().kind === "tool_call"}>{ToolCallPart(part)}</Match>
            <Match when={part().kind === "tool_result"}>{ToolResultPart(part)}</Match>
          </Switch>
        )}
      </Index>
      <Show when={finish()}>
        {(f) => (
          <box paddingLeft={3} marginTop={1} flexDirection="row" gap={1}>
            <text fg={props.accent}>▣</text>
            <text fg={oc.textMuted} wrapMode="none">
              {props.model}
              {f().cost ? ` · ${formatUsd(f().cost)}` : ""}
            </text>
          </box>
        )}
      </Show>
    </box>
  );
}
