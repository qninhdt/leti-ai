// Session sidebar (width 42 when terminal is wide). Shows session/agent/cost
// summary. Uses fine-grained selectors to only re-render on relevant changes.

import { createMemo, Show } from "solid-js";

import { theme } from "../theme/index.js";
import { useStoreSelector } from "../render/store-bridge.js";
import { shortId } from "../utils/format.js";

export function Sidebar() {
  const oc = theme.oc;
  const activeSessionId = useStoreSelector((s) => s.activeSessionId);
  const sessions = useStoreSelector((s) => s.sessions);
  const agents = useStoreSelector((s) => s.agents);

  const session = createMemo(() => {
    const id = activeSessionId();
    return id ? sessions()[id] ?? null : null;
  });
  const agent = createMemo(() => {
    const s = session();
    return s ? agents().find((a) => a.id === s.agent_id) ?? null : null;
  });

  return (
    <box flexDirection="column" width={42} paddingLeft={2} paddingTop={1} gap={1}>
      <text fg={oc.textMuted}>SESSION</text>
      <Show when={session()} fallback={<text fg={oc.textMuted}>no active session</text>}>
        {(s) => (
          <box flexDirection="column">
            <text fg={oc.text}>{shortId(s().id)}</text>
            <text fg={oc.textMuted}>{s().status}</text>
            <text fg={oc.textMuted}>{s().permission_mode}</text>
            <Show when={agent()}>
              {(a) => (
                <box flexDirection="column" marginTop={1}>
                  <text fg={oc.text}>{a().display_name}</text>
                  <Show when={a().model}>
                    {(m) => <text fg={oc.textMuted}>{m()}</text>}
                  </Show>
                </box>
              )}
            </Show>
            <text fg={oc.warning}>${s().cost_decimal_str}</text>
          </box>
        )}
      </Show>
    </box>
  );
}
