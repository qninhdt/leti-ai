// Session route: the active-session screen. A row of [main column + optional
// sidebar]. Main column = a sticky-bottom <scrollbox> of messages (rendered by
// MessageList) above the footer area (the prompt editor). Sidebar shows inline
// when the terminal is wide (>120). Narrow-mode session detail
// (sidebar-as-overlay) is deferred to Phase 5; for now narrow terminals simply
// omit the sidebar.

import { Show, createMemo, createEffect } from "solid-js";

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

  // --- Auto-scroll (sticky-to-bottom that survives streaming) ---------------
  // opentui's built-in stickyScroll latches an internal "manual scroll" flag
  // when content grows faster than one line (each part_delta), and then stops
  // following. We re-assert bottom-follow ourselves, but only while the user
  // hasn't scrolled up to read history.
  //
  // Invariant that makes this timing-robust: content growth changes
  // scrollHeight (→ maxScrollTop) but NEVER scrollTop. So if scrollTop still
  // equals the value we last set, the change was content (keep following); if
  // it differs, the USER moved it (follow only when they returned to bottom).
  let scrollRef: ScrollBoxRenderable | undefined;
  let lastSetTop = 0;
  let autoFollow = true;

  // Cheap signature that changes on every auto-scroll trigger: a new message,
  // a new part, or the last part's streamed buffers/text growing.
  const contentSignature = createMemo(() => {
    const list = messages();
    const last = list[list.length - 1];
    const lastPart = last?.parts[last.parts.length - 1];
    const grow = lastPart
      ? lastPart.buffer.length + lastPart.reasoning_buffer.length + (lastPart.text?.length ?? 0)
      : 0;
    return `${list.length}:${last?.parts.length ?? 0}:${grow}`;
  });

  createEffect(() => {
    contentSignature(); // track content growth
    // Defer past opentui's layout pass so scrollHeight reflects the new content.
    queueMicrotask(() => {
      const box = scrollRef;
      if (!box) return;
      const maxTop = Math.max(0, box.scrollHeight - box.viewport.height);
      if (Math.abs(box.scrollTop - lastSetTop) > 1) {
        // scrollTop moved to a value WE didn't set → the user scrolled. Keep
        // following only if they parked at the bottom; otherwise let them read.
        autoFollow = box.scrollTop >= maxTop - 1;
      }
      if (autoFollow) {
        // Scroll to a sentinel; the scrollbar clamps to the true bottom even if
        // our maxTop is a frame stale, then read the clamped value back so the
        // "did the user scroll" check above stays accurate next tick.
        box.scrollTo({ x: box.scrollLeft, y: Number.MAX_SAFE_INTEGER });
        lastSetTop = box.scrollTop;
      } else {
        lastSetTop = box.scrollTop;
      }
    });
  });

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
