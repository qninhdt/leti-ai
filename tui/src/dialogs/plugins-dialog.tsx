// Plugins overlay content. Lists installed
// plugins with enabled state, version, and capabilities. Pure content — no key
// handler, so the router's Esc-pops-overlay path closes it (the old view had no
// exit). Plugin *errors* surface separately as toasts (see toast-host).

import { For, Show } from "solid-js";

import { theme } from "../theme/index.js";
import { useStoreSelector } from "../render/store-bridge.js";

export function PluginsDialog() {
  const oc = theme.oc;
  const plugins = useStoreSelector((s) => s.plugins);

  return (
    <box flexDirection="column" minWidth={42}>
      <text fg={oc.primary}>Plugins</text>
      <Show when={plugins().length > 0} fallback={<text fg={oc.textMuted}>(none reported)</text>}>
        <For each={plugins()}>
          {(p) => (
            <box flexDirection="column">
              <box flexDirection="row">
                <text fg={p.enabled ? oc.success : oc.textMuted}>{p.enabled ? "● " : "○ "}</text>
                <text fg={oc.text}>{p.name}</text>
                <text fg={oc.textMuted}> v{p.version}</text>
              </box>
              <text fg={oc.textMuted}>  capabilities: {p.capabilities.join(", ") || "—"}</text>
            </box>
          )}
        </For>
      </Show>
      <text fg={oc.textMuted}>Esc to dismiss.</text>
    </box>
  );
}
