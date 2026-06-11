// Root Solid component. Mirrors OpenCode's app.tsx shell: a full-screen root
// column holding the active route (home when no session, session otherwise), an
// absolute overlay layer for dialogs, and a startup-loading gate. Bootstrap +
// SSE wiring and the global key router are installed here once. The footer/
// prompt editor and message rendering are filled by Phases 3-4; this phase
// establishes the layout, routing, and overlay infrastructure.

import { Show, createMemo } from "solid-js";

import { HomeRoute } from "./routes/home-route.js";
import { SessionRoute } from "./routes/session-route.js";
import { OverlayHost } from "./render/overlay-host.js";
import { useKeyRouter } from "./render/key-router.js";
import { useBootstrap } from "./render/use-bootstrap.js";
import { useStoreSelector } from "./render/store-bridge.js";
import { useTerminalSize } from "./hooks/use-terminal-size.js";
import { theme } from "./theme/index.js";

export function App() {
  const oc = theme.oc;
  const size = useTerminalSize();
  const activeSessionId = useStoreSelector((s) => s.activeSessionId);

  useBootstrap();
  useKeyRouter();

  const hasSession = createMemo(() => activeSessionId() !== null);

  return (
    <box
      flexDirection="column"
      width={size().width}
      height={size().height}
      backgroundColor={oc.background}
    >
      <box flexGrow={1} minHeight={0}>
        <Show when={hasSession()} fallback={<HomeRoute />}>
          <SessionRoute width={size().width} />
        </Show>
      </box>
      <OverlayHost />
    </box>
  );
}
