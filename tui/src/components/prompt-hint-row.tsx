// The prompt's hint row, ported from OpenCode's `component/prompt/index.tsx`
// (the line under the `▀` shelf). Left: a spinner + "working…" while the turn
// streams, with `esc interrupt` / `esc again to interrupt` reflecting the
// interrupt arm count; otherwise a muted idle label. Right (idle only): the
// `tokens (pct) · $cost` usage summary plus the `⌘K commands` shortcut. The
// retry/workspace/warp states OpenCode shows are out of scope here (no data
// source on the Leti backend), so the running state is the single busy view.

import { Show, Switch, Match } from "solid-js";
import "opentui-spinner/solid";

import { theme } from "../theme/index.js";

export interface PromptHintRowProps {
  /// Whether the active session is mid-turn (drives the spinner + interrupt UX).
  streaming: boolean;
  /// Interrupt arm count: 0 = not armed, >0 = "esc again to interrupt" primed.
  interruptArmed: boolean;
  /// Idle-state left label (e.g. workspace/cwd hint). Shown when not streaming.
  idleLabel?: string;
  /// Combined `tokens (pct)` usage string, or undefined when no usage yet.
  usage?: string;
  /// Formatted session cost (e.g. "$0.0312"), or undefined when zero/unknown.
  cost?: string;
  /// Display label for the command-palette shortcut (⌘K or ctrl+k fallback).
  paletteShortcut: string;
  /// Spinner rail color (matches the editor's left bar / agent accent).
  spinnerColor: string;
}

export function PromptHintRow(props: PromptHintRowProps) {
  const oc = theme.oc;
  const usageText = () => [props.usage, props.cost].filter(Boolean).join(" · ");

  return (
    <box width="100%" flexDirection="row" justifyContent="space-between">
      <Switch>
        <Match when={props.streaming}>
          <box flexDirection="row" gap={1} flexGrow={1} justifyContent="space-between">
            <box flexShrink={0} flexDirection="row" gap={1}>
              <box marginLeft={1}>
                <spinner color={props.spinnerColor} />
              </box>
              <text fg={oc.textMuted}>working…</text>
            </box>
            <text fg={props.interruptArmed ? oc.primary : oc.text}>
              esc{" "}
              <span style={{ fg: props.interruptArmed ? oc.primary : oc.textMuted }}>
                {props.interruptArmed ? "again to interrupt" : "interrupt"}
              </span>
            </text>
          </box>
        </Match>
        <Match when={true}>
          <Show when={props.idleLabel} fallback={<text />}>
            {(label) => (
              <box paddingLeft={3}>
                <text fg={oc.textMuted}>{label()}</text>
              </box>
            )}
          </Show>
        </Match>
      </Switch>
      <Show when={!props.streaming}>
        <box gap={2} flexDirection="row">
          <Show when={usageText()}>
            {(text) => (
              <text fg={oc.textMuted} wrapMode="none">
                {text()}
              </text>
            )}
          </Show>
          <text fg={oc.text}>
            {props.paletteShortcut} <span style={{ fg: oc.textMuted }}>commands</span>
          </text>
        </box>
      </Show>
    </box>
  );
}
