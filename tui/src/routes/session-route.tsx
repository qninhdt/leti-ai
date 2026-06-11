// Session route: the active-session screen. A row of [main column + optional
// sidebar]. Main column = a sticky-bottom <scrollbox> of messages (Phase 4
// fills MessageList) above the footer area (Phase 3 fills the real editor).
// Sidebar shows inline when the terminal is wide (>120). Narrow-mode session
// detail (sidebar-as-overlay) is deferred to Phase 5; for now narrow terminals
// simply omit the sidebar.

import { Show, For } from "solid-js";

import { theme } from "../theme/index.js";
import { useStoreSelector } from "../render/store-bridge.js";
import { Sidebar } from "../components/sidebar.js";
import { FooterArea } from "../components/footer-area.js";

import type { MessageView } from "../store/index.js";

const SIDEBAR_WIDTH = 42;
const WIDE_THRESHOLD = 120;

// Shared empty reference so the messages selector's Object.is gate can
// short-circuit on the no-messages path instead of allocating a fresh [].
const EMPTY: MessageView[] = [];

export interface SessionRouteProps {
  /// Current terminal width — drives the wide/narrow sidebar decision.
  width: number;
}

export function SessionRoute(props: SessionRouteProps) {
  const oc = theme.oc;
  const wide = () => props.width > WIDE_THRESHOLD;
  const messages = useStoreSelector((s) => {
    const id = s.activeSessionId;
    return id ? s.messages[id] ?? EMPTY : EMPTY;
  });

  return (
    <box flexDirection="row" flexGrow={1} minHeight={0}>
      <box flexDirection="column" flexGrow={1} minHeight={0} paddingLeft={2} paddingRight={2} paddingBottom={1}>
        <scrollbox stickyScroll={true} stickyStart="bottom" flexGrow={1}>
          <box height={1} />
          <For each={messages()}>
            {(msg) => (
              <box paddingLeft={3} marginTop={1}>
                <text fg={oc.textMuted}>
                  {msg.role}: {msg.parts.length} part(s)
                </text>
              </box>
            )}
          </For>
        </scrollbox>
        <FooterArea />
      </box>
      <Show when={wide()}>
        <box width={SIDEBAR_WIDTH} flexShrink={0}>
          <Sidebar />
        </box>
      </Show>
    </box>
  );
}
