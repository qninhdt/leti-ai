// Assistant message, ported from OpenCode's assistant branch
// (`routes/session/index.tsx:1402`). Iterates the message's parts through a
// kind-keyed dispatch (text / reasoning / tool) and renders a per-turn footer
// line `▣ mode · model · cost` once `step_finish` arrives. NO duration — the
// step_finished DTO carries reason/usage/cost only, no timing — so cost is the
// trailing metric. A tool_result part is folded into its tool_call's block by
// matching tool_call_id, never rendered as a standalone row.

import { For, Show, createMemo } from "solid-js";

import { theme } from "../theme/index.js";
import { formatUsd } from "../utils/format.js";
import { toolVisual } from "./tool-visuals.js";
import { PartText } from "./part-text.js";
import { PartReasoning } from "./part-reasoning.js";
import { ToolInline } from "./tool-inline.js";
import { ToolBlock } from "./tool-block.js";

import type { MessageView, PartView } from "../store/index.js";

export interface MessageAssistantProps {
  message: MessageView;
  /// Agent accent for the ▣ footer glyph.
  accent: string;
  /// Model label for the footer (agent's model, or a dash).
  model: string;
  /// Permission mode label for the footer.
  mode: string;
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

  const renderPart = (part: PartView) => {
    switch (part.kind) {
      case "text":
        return <PartText part={part} />;
      case "reasoning":
        return <PartReasoning part={part} />;
      case "tool_call": {
        const visual = toolVisual(part.tool_name);
        if (visual.template === "block") {
          const byId = resultByCallId();
          const out =
            (part.tool_call_id ? byId.get(part.tool_call_id) : undefined) ??
            (part.id ? byId.get(part.id) : undefined) ??
            part.tool_result;
          return (
            <ToolBlock part={part} title={`# ${part.tool_name ?? "tool"}`} output={resultText(out)} />
          );
        }
        return <ToolInline part={part} summary={`${part.tool_name ?? "tool"} ${summarize(part.tool_args)}`.trim()} />;
      }
      case "tool_result":
        // Suppress a result row only when a BLOCK tool_call will fold it into
        // its card. Inline-tool results, and orphans with no matching call,
        // still render as their own line so output is never silently dropped.
        return (
          <Show when={!part.tool_call_id || !blockCallIds().has(part.tool_call_id)}>
            <ToolInline part={part} summary={resultText(part.tool_result).slice(0, 80)} />
          </Show>
        );
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
              {props.mode} · {props.model}
              {f().cost ? ` · ${formatUsd(f().cost)}` : ""}
            </text>
          </box>
        )}
      </Show>
    </box>
  );
}
