// Permission requests replace the composer in-place. The request remains in
// the footer until its authoritative permission_resolved event arrives.

import { Show, createMemo } from "solid-js";

import { useStoreSelector } from "../render/store-bridge.js";
import { useStore } from "../store/index.js";
import { PermissionFooter } from "./permission-footer.js";
import { findSessionPermission } from "./permission-footer-selection.js";
import { PromptEditor } from "./prompt-editor.js";
import { SubagentFooter } from "./subagent-footer.js";

export function FooterArea() {
  const pending = useStoreSelector((s) => s.pendingPermissions);
  const activeSessionId = useStoreSelector((s) => s.activeSessionId);
  const sessions = useStoreSelector((s) => s.sessions);
  const subagents = useStoreSelector((s) => s.subagents);
  const active = createMemo(() => findSessionPermission(pending(), activeSessionId()));
  const child = createMemo(() => {
    const id = activeSessionId();
    return id ? sessions()[id] : undefined;
  });
  const childTask = createMemo(() =>
    Object.values(subagents()).find((task) => task.child_session_id === activeSessionId()),
  );
  const siblings = createMemo(() => {
    const parent = child()?.parent_session_id;
    return parent
      ? Object.values(sessions())
          .filter((session) => session.parent_session_id === parent)
          .sort((a, b) => a.created_at.localeCompare(b.created_at))
      : [];
  });
  const childIndex = createMemo(() => siblings().findIndex((session) => session.id === activeSessionId()));

  return (
    <Show when={active()} keyed fallback={
      <Show
        when={child()?.parent_session_id}
        fallback={<PromptEditor />}
      >
        <SubagentFooter
          agent={childTask()?.agent}
          siblingIndex={childIndex()}
          siblingTotal={siblings().length}
          cost={childTask()?.cost}
          parentSessionId={child()?.parent_session_id ?? undefined}
          previousSessionId={
            childIndex() > 0 ? siblings()[childIndex() - 1]?.id : undefined
          }
          nextSessionId={
            childIndex() >= 0 ? siblings()[childIndex() + 1]?.id : undefined
          }
          onNavigate={(sessionId) => useStore.getState().setActiveSession(sessionId)}
        />
      </Show>
    }>
      {(request) => <PermissionFooter request={request} />}
    </Show>
  );
}
