// Session route: the active-session screen. A row of [main column + optional
// sidebar]. Main column = a sticky-bottom <scrollbox> of messages (rendered by
// MessageList) above the footer area (the prompt editor). Sidebar shows inline
// when the terminal is wide (>120). Narrow-mode session detail
// (sidebar-as-overlay) is deferred to Phase 5; for now narrow terminals simply
// omit the sidebar.

import { Show, createMemo, createEffect, on } from "solid-js";

import { theme } from "../theme/index.js";
import { useStoreSelector } from "../render/store-bridge.js";
import { Sidebar } from "../components/sidebar.js";
import { FooterArea } from "../components/footer-area.js";
import { MessageList } from "../components/message-list.js";

import type { ScrollBoxRenderable } from "@opentui/core";
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
  const wide = () => props.width > WIDE_THRESHOLD;

  const activeSessionId = useStoreSelector((s) => s.activeSessionId);
  const sessions = useStoreSelector((s) => s.sessions);
  const agents = useStoreSelector((s) => s.agents);
  const planModes = useStoreSelector((s) => s.planMode);
  const messages = useStoreSelector((s) => {
    const id = s.activeSessionId;
    return id ? s.messages[id] ?? EMPTY : EMPTY;
  });

  const session = createMemo(() => {
    const id = activeSessionId();
    return id ? sessions()[id] ?? null : null;
  });
  const agent = createMemo(() => {
    const s = session();
    return s ? agents().find((a) => a.id === s.agent_id) ?? null : null;
  });
  const accent = createMemo(() => (agent() ? theme.oc.borderActive : theme.oc.border));
  const model = createMemo(() => agent()?.model ?? "—");
  const planMode = createMemo(() => {
    const id = activeSessionId();
    return id ? !!planModes()[id] : false;
  });

  // --- Auto-scroll --------------------------------------------------------
  // The <scrollbox> is configured stickyScroll + stickyStart="bottom", which
  // follows the bottom edge on its own as content grows during streaming: its
  // content-resize handler re-pins to the bottom on every size change unless
  // the user has manually scrolled up. So we do NOT re-assert scroll per token
  // — doing that forces a full viewport relayout each frame (the streaming
  // flicker). We only jump to the bottom once when switching into a session,
  // to land on the newest message.
  let scrollRef: ScrollBoxRenderable | undefined;

  createEffect(
    on(activeSessionId, () => {
      // Defer past opentui's layout pass so scrollHeight reflects the content.
      queueMicrotask(() => {
        const box = scrollRef;
        if (box) box.scrollTo({ x: box.scrollLeft, y: Number.MAX_SAFE_INTEGER });
      });
    }),
  );

  return (
    <box flexDirection="row" flexGrow={1} minHeight={0}>
      <box flexDirection="column" flexGrow={1} minHeight={0} paddingLeft={2} paddingRight={2} paddingBottom={1}>
        <scrollbox
          ref={(r: ScrollBoxRenderable) => {
            scrollRef = r;
          }}
          stickyScroll={true}
          stickyStart="bottom"
          flexGrow={1}
        >
          <box height={1} />
          <MessageList
            messages={messages()}
            accent={accent()}
            model={model()}
            planMode={planMode()}
          />
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
