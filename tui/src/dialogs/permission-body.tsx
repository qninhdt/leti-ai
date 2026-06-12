// Permission body renderer — displays the diff/bash/generic detail for a
// permission request. Extracted from permission-dialog so the dialog file
// focuses on the keyboard state machine.

import { Show } from "solid-js";
import { createPatch } from "diff";

import { theme } from "../theme/index.js";

import type { PermissionRequestDto } from "../api/types.js";

function diffColor(line: string): string {
  const oc = theme.oc;
  if (line.startsWith("+") && !line.startsWith("+++")) return oc.diffAdd;
  if (line.startsWith("-") && !line.startsWith("---")) return oc.diffDelete;
  return oc.textMuted;
}

export function PermissionBody(props: { request: PermissionRequestDto }) {
  const oc = theme.oc;
  const diff = () => props.request.diff;
  const bash = () => props.request.bash;
  const patchLines = () => {
    const d = diff();
    if (!d) return [];
    return createPatch(d.filepath, d.before, d.after, "", "").split("\n").slice(4);
  };

  return (
    <Show
      when={diff()}
      fallback={
        <Show
          when={bash()}
          fallback={
            <box marginTop={1}>
              <text fg={oc.textMuted}>
                {props.request.tool_name}
                {props.request.reason ? ` — ${props.request.reason}` : ""}
              </text>
            </box>
          }
        >
          {(b) => (
            <box marginTop={1} flexDirection="column">
              <text fg={oc.text}>
                <span style={{ fg: oc.textMuted }}>$ </span>
                {b().command}
              </text>
              <text fg={oc.textMuted}>
                cwd: {b().cwd} · timeout: {b().timeout_ms}ms
              </text>
            </box>
          )}
        </Show>
      }
    >
      {(d) => (
        <box marginTop={1} flexDirection="column">
          <text fg={oc.textMuted}>{d().filepath}</text>
          {patchLines().map((line) => (
            <text fg={diffColor(line)} wrapMode="none">
              {line}
            </text>
          ))}
        </box>
      )}
    </Show>
  );
}
