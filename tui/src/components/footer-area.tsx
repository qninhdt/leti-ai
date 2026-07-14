// Permission requests replace the composer in-place. The request remains in
// the footer until its authoritative permission_resolved event arrives.

import { Show, createMemo } from "solid-js";

import { useStoreSelector } from "../render/store-bridge.js";
import { PermissionFooter } from "./permission-footer.js";
import { findSessionPermission } from "./permission-footer-selection.js";
import { PromptEditor } from "./prompt-editor.js";

export function FooterArea() {
  const pending = useStoreSelector((s) => s.pendingPermissions);
  const activeSessionId = useStoreSelector((s) => s.activeSessionId);
  const active = createMemo(() => findSessionPermission(pending(), activeSessionId()));

  return (
    <Show when={active()} keyed fallback={<PromptEditor />}>
      {(request) => <PermissionFooter request={request} />}
    </Show>
  );
}
