// Toast / banner host, rendered above the route. Two distinct surfaces with
// deliberately different lifecycles:
//   - Plugin errors: TRANSIENT toasts from the store's capped ring buffer. Each
//     newly-seen error shows briefly then auto-dismisses; this is non-critical,
//     informational noise.
//   - clientError: a PERSISTENT banner that does NOT auto-dismiss. It is the
//     crash-visibility guard for failed prompt/command calls (an async failure
//     in a sync key handler would otherwise be an invisible unhandled
//     rejection). It clears only on the next successful submit (store logic).
// Mounted once near the root so it floats over whichever route is active.

import { For, Show, createSignal, createEffect, onCleanup } from "solid-js";

import { theme } from "../theme/index.js";
import { useStoreSelector } from "./store-bridge.js";

import type { PluginErrorView } from "../store/index.js";

const TOAST_TTL_MS = 5000;

export function ToastHost() {
  const oc = theme.oc;
  const pluginErrors = useStoreSelector((s) => s.pluginErrors);
  const clientError = useStoreSelector((s) => s.clientError);

  // Track which plugin errors have already been surfaced, by OBJECT IDENTITY
  // (the store creates each entry once and keeps the same reference in its ring
  // buffer). This `seen` set is durable and decoupled from `visible`: dismissing
  // a toast must NOT make the effect treat its error as new again, or the toast
  // resurrects every TTL because the error is still in the store buffer.
  // Reference identity also avoids collapsing two same-millisecond errors that
  // happen to share an `at` timestamp.
  const [visible, setVisible] = createSignal<PluginErrorView[]>([]);
  const seen = new Set<PluginErrorView>();
  const timers = new Set<ReturnType<typeof setTimeout>>();

  createEffect(() => {
    const fresh = pluginErrors().filter((e) => !seen.has(e));
    if (fresh.length === 0) return;
    for (const e of fresh) seen.add(e);
    setVisible((prev) => [...prev, ...fresh]);
    for (const entry of fresh) {
      const t = setTimeout(() => {
        setVisible((prev) => prev.filter((e) => e !== entry));
        timers.delete(t);
      }, TOAST_TTL_MS);
      timers.add(t);
    }
  });

  onCleanup(() => {
    for (const t of timers) clearTimeout(t);
    timers.clear();
  });

  return (
    <box flexDirection="column" gap={1}>
      <Show when={clientError()}>
        {(err) => (
          <box
            border={["left"]}
            borderColor={oc.error}
            backgroundColor={oc.backgroundPanel}
            paddingLeft={2}
            paddingRight={2}
          >
            <text fg={oc.error}>{err()}</text>
          </box>
        )}
      </Show>
      <For each={visible()}>
        {(entry) => (
          <box
            border={["left"]}
            borderColor={oc.warning}
            backgroundColor={oc.backgroundPanel}
            paddingLeft={2}
            paddingRight={2}
          >
            <text fg={oc.warning}>
              {entry.pluginId}: {entry.message}
            </text>
          </box>
        )}
      </For>
    </box>
  );
}
