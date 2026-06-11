// Phase-1 spike screen. Exercises the Go/No-Go-critical element set so a
// successful build + clean mount proves the engine works under Solid:
// <box>/<text>/<textarea>/<scrollbox>, an absolute overlay with a dim
// backdrop, a left-only `┃` border, a <spinner>, and the zustand->Solid
// bridge (the header reflects conn status mutated by a timer). The rich
// <markdown>/<diff>/<code> elements are probed by typecheck only here; their
// runtime wiring belongs to Phase 4 message rendering.

import { createSignal, onCleanup, Show, For, ErrorBoundary } from "solid-js";
import { writeFileSync } from "node:fs";
import "opentui-spinner/solid";

import { theme } from "../theme/index.js";
import { useStoreSelector } from "./store-bridge.js";
import { useStore } from "../store/index.js";

function Body() {
  const oc = theme.oc;
  const [overlay, setOverlay] = createSignal(false);
  const [ticks, setTicks] = createSignal(0);
  const connStatus = useStoreSelector((s) => s.conn.status);

  const timer = setInterval(() => {
    setTicks((n) => n + 1);
    useStore.getState().setConn(ticks() % 2 === 0 ? "open" : "reconnecting");
    if (ticks() === 3) setOverlay(true);
  }, 500);
  onCleanup(() => clearInterval(timer));

  const lines = Array.from({ length: 40 }, (_, i) => `scroll row ${i + 1}`);

  return (
    <box flexDirection="column" backgroundColor={oc.background} flexGrow={1}>
      <box border={["left"]} borderColor={oc.borderActive} paddingLeft={2} paddingTop={1}>
        <text fg={oc.text}>spike tick {ticks()} conn={connStatus()}</text>
      </box>
      <scrollbox stickyScroll={true} stickyStart="bottom" flexGrow={1}>
        <For each={lines}>{(l) => <text fg={oc.textMuted}>{l}</text>}</For>
      </scrollbox>
      <box border={["left"]} borderColor={oc.border} paddingLeft={2}>
        <spinner color={oc.primary} />
        <textarea placeholder="type here..." placeholderColor={oc.textMuted} textColor={oc.text} minHeight={1} maxHeight={6} />
      </box>
      <Show when={overlay()}>
        <box position="absolute" left={0} top={0} right={0} bottom={0} backgroundColor="#00000046" justifyContent="center" alignItems="center">
          <box border={["left"]} borderColor={oc.borderActive} backgroundColor={oc.backgroundPanel} paddingLeft={2} paddingTop={1} paddingBottom={1}>
            <text fg={oc.text}>overlay atop route with dim backdrop</text>
          </box>
        </box>
      </Show>
    </box>
  );
}

export function SpikeScreen() {
  return (
    <ErrorBoundary
      fallback={(err) => {
        writeFileSync("/tmp/spike-error.txt", String(err?.stack ?? err));
        return <text fg="#ff5f5f">spike error: {String(err?.message ?? err)}</text>;
      }}
    >
      <Body />
    </ErrorBoundary>
  );
}
