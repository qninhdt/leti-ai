// Session sidebar (width 42 when terminal is wide). Shows session/agent/cost
// summary. Phase 5 fills the full detail (plugin count, permission mode, etc.);
// here it renders the core fields the store already exposes so the layout
// column is real, not an empty reserve.

import { Show } from "solid-js";

import { theme } from "../theme/index.js";
import { useStoreSnapshot } from "../render/store-bridge.js";
import { shortId } from "../utils/format.js";

export function Sidebar() {
  const oc = theme.oc;
  const snap = useStoreSnapshot();

  const session = () => {
    const id = snap().activeSessionId;
    return id ? snap().sessions[id] ?? null : null;
  };
  const agent = () => {
    const s = session();
    return s ? snap().agents.find((a) => a.id === s.agent_id) ?? null : null;
  };

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
                  <text fg={oc.text}>{a().name}</text>
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
