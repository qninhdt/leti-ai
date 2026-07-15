// Session picker overlay content. Lists sessions sorted by updated_at
// Lists sessions sorted by updated_at (newest first) as `shortId · status ·
// permission_mode` rows; selecting one makes it active and closes the overlay.
// Navigation runs through the shared list-nav helper on the key router's
// overlay seam; Escape falls through to the router's pop.

import { For, Show, createMemo, onCleanup, onMount } from "solid-js";

import { theme } from "../theme/index.js";
import { useStore } from "../store/index.js";
import { useStoreSelector } from "../render/store-bridge.js";
import { setOverlayHandler } from "../render/key-router.js";
import { shortId } from "../utils/format.js";
import { createListNav } from "./use-list-nav.js";

import type { SessionDto } from "../api/types.js";

export function SessionPickerDialog() {
  const oc = theme.oc;
  const sessions = useStoreSelector((s) => s.sessions);
  const ordered = createMemo(() =>
    Object.values(sessions())
      .filter((session) => !session.parent_session_id)
      .sort((a, b) => b.updated_at.localeCompare(a.updated_at)),
  );

  function select(session: SessionDto): void {
    const store = useStore.getState();
    store.removeOverlay((e) => e.kind === "session_picker");
    store.setActiveSession(session.id);
  }

  const nav = createListNav(ordered, select);
  onMount(() => setOverlayHandler(nav.handler));
  onCleanup(() => setOverlayHandler(null));

  return (
    <box flexDirection="column" minWidth={42}>
      <text fg={oc.primary}>Sessions</text>
      <Show when={ordered().length > 0} fallback={<text fg={oc.textMuted}>(no sessions yet)</text>}>
        <For each={ordered()}>
          {(session, i) => (
            <text fg={i() === nav.index() ? oc.accent : oc.text}>
              {i() === nav.index() ? "▸ " : "  "}
              {shortId(session.id)} · {session.status} · {session.permission_mode}
            </text>
          )}
        </For>
      </Show>
      <text fg={oc.textMuted}>↑↓ select · Enter resume · Esc cancel</text>
    </box>
  );
}
