// InlineTool one-liner, ported from OpenCode's `InlineTool`
// (`routes/session/index.tsx:1730`). A single `paddingLeft=3` row: the tool's
// icon (from tool-visuals) followed by a short content summary. Icon color
// follows state — complete→muted, running→text, errored→error. A pending
// permission tint is Phase 5's concern (overlay handles it), so here we color
// by the part's own status only.

import { theme } from "../theme/index.js";
import { toolVisual } from "./tool-visuals.js";

import type { PartView } from "../store/index.js";

export interface ToolInlineProps {
  part: PartView;
  /// One-line summary to the right of the icon (e.g. "Read src/foo.ts").
  summary: string;
}

export function ToolInline(props: ToolInlineProps) {
  const oc = theme.oc;
  const icon = () => toolVisual(props.part.tool_name).icon;
  const fg = () => {
    if (props.part.status === "errored") return oc.error;
    if (props.part.status === "complete") return oc.textMuted;
    return oc.text;
  };

  return (
    <box paddingLeft={3} flexDirection="row" gap={1}>
      <text fg={fg()}>{icon()}</text>
      <text fg={fg()} wrapMode="none">
        {props.summary}
      </text>
    </box>
  );
}
