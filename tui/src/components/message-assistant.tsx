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

import { For, Show, createMemo } from "solid-js";

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
import { parseFileDiff } from "./tool-diff-parse.js";
import { ToolTodo } from "./tool-todo.js";
import { ToolAskUser } from "./tool-ask-user.js";
import { CompactionDivider } from "./compaction-divider.js";

import type { MessageView, PartView } from "../store/index.js";

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

  const renderPart = (part: PartView) => {
    switch (part.kind) {
      case "text":
        return <PartText part={part} />;
      case "reasoning":
        return <PartReasoning part={part} />;
      case "compaction":
        return <CompactionDivider part={part} />;
      case "tool_call": {
        const name = (part.tool_name ?? "").toLowerCase();
        // Todo renders as a checklist from its args, not a generic tool line.
        if (name === "todo") {
          return <ToolTodo part={part} />;
        }
        // ask_user renders as a question block (options + chosen answer)
        // instead of raw JSON. The live selection UI is a separate overlay.
        if (name === "ask_user") {
          const byId = resultByCallId();
          const out =
            (part.tool_call_id ? byId.get(part.tool_call_id) : undefined) ??
            (part.id ? byId.get(part.id) : undefined) ??
            part.tool_result;
          return <ToolAskUser part={part} result={out} />;
        }
        const visual = toolVisual(part.tool_name);
        if (visual.template === "block") {
          const byId = resultByCallId();
          const out =
            (part.tool_call_id ? byId.get(part.tool_call_id) : undefined) ??
            (part.id ? byId.get(part.id) : undefined) ??
            part.tool_result;
          const title = `# ${toolBlockTitle(part.tool_name, part.tool_args)}`;
          // edit/write emit a structured FileDiff in their result body; render
          // it as a colored diff card. A write "create" has no diff and falls
          // through to the generic block card below.
          const diff = parseFileDiff(out);
          if (diff) {
            return <ToolDiff part={part} title={title} diff={diff} />;
          }
          return (
            <ToolBlock
              part={part}
              title={title}
              output={formatToolOutput(part.tool_name, resultText(out))}
            />
          );
        }
        return (
          <ToolInline
            part={part}
            summary={`${part.tool_name ?? "tool"} ${toolLabel(part.tool_name, part.tool_args)}`.trim()}
          />
        );
      }
      case "tool_result": {
        // Suppress a result row when a BLOCK tool_call folds it into its card,
        // or when the todo checklist already renders the call. Inline-tool
        // results, and orphans with no matching call, still render so output is
        // never silently dropped.
        const name = part.tool_call_id ? toolNameByCallId().get(part.tool_call_id) : undefined;
        const formatted = formatToolOutput(name, resultText(part.tool_result));
        return (
          <Show when={!part.tool_call_id || !suppressedResultIds().has(part.tool_call_id)}>
            <ToolInline part={part} summary={formatted.split("\n")[0]?.slice(0, 80) ?? ""} />
          </Show>
        );
      }
      default:
        return null;
    }
  };

  const finish = () => props.message.step_finish;

  return (
    <box flexDirection="column">
      <For each={props.message.parts}>{(part) => renderPart(part)}</For>
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
