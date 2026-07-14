// Child-session footer, shown when the user has navigated INTO a subagent's
// session. Mirrors OpenCode's subagent-footer.tsx: agent label + sibling
// position `(i of n)` + live usage. The label-derivation is a pure exported
// helper (`deriveAgentLabel`) so it's unit-testable without the Solid runtime.

import { theme } from "../theme/index.js";

/// Derive the agent label for a child session's footer. OpenCode titles a
/// subagent session `@<slug> subagent`; we prefer an explicit `agent` slug
/// when present and fall back to parsing the title. Returns a bare slug (no
/// `@`) or "subagent" when nothing resolves.
export function deriveAgentLabel(agent: string | undefined, title: string | undefined): string {
  if (agent && agent.trim().length > 0) return agent.trim();
  if (title) {
    const m = title.match(/@(\w[\w-]*)\s+subagent/i);
    if (m?.[1]) return m[1];
  }
  return "subagent";
}

/// Format the sibling position as `(i of n)`, 1-indexed. Returns an empty
/// string when the index is out of range or there are no siblings, so the
/// footer degrades gracefully rather than showing `(0 of 0)`.
export function formatSiblingPosition(index: number, total: number): string {
  if (total <= 0 || index < 0 || index >= total) return "";
  return `(${index + 1} of ${total})`;
}

export interface SubagentFooterProps {
  /// Agent slug for the child (preferred over title parsing).
  agent?: string;
  /// Child session title (fallback source for the label).
  title?: string;
  /// 0-based index of this child among its siblings.
  siblingIndex: number;
  /// Total sibling count.
  siblingTotal: number;
  /// Rendered cost string (e.g. "0.0100"), if any.
  cost?: string;
}

export function SubagentFooter(props: SubagentFooterProps) {
  const oc = theme.oc;
  const label = () => deriveAgentLabel(props.agent, props.title);
  const pos = () => formatSiblingPosition(props.siblingIndex, props.siblingTotal);
  return (
    <box flexDirection="row" gap={1} paddingLeft={1} paddingRight={1}>
      <text fg={oc.accent}>{`@${label()}`}</text>
      <text fg={oc.textMuted}>{pos()}</text>
      <box flexGrow={1} />
      <text fg={oc.textMuted}>{props.cost ? `$${props.cost}` : ""}</text>
    </box>
  );
}
