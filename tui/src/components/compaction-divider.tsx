// Transcript divider marking where the conversation was compacted. When
// context pressure fires auto-compaction (or the user runs /compact), the older
// messages are superseded by a summary the model sees in their place. The
// transcript still scrolls through the original messages above this line; the
// divider just marks the boundary and reports how much was folded away, so the
// history doesn't appear to silently lose turns. Rendered from the
// `Part::Compaction` marker (summary + original_token_count) via hydration.

import { theme } from "../theme/index.js";
import { formatTokens } from "../utils/format.js";

import type { PartView } from "../store/index.js";

export interface CompactionDividerProps {
  part: PartView;
}

export function CompactionDivider(props: CompactionDividerProps) {
  const oc = theme.oc;

  // original_token_count is the heuristic size of the pre-compaction window;
  // 0 when a rolled-back/empty summary was persisted (never surfaced to the
  // user as a real compaction), so the count is dropped in that case.
  const label = () => {
    const n = props.part.original_token_count ?? 0;
    return n > 0 ? `context compacted · ~${formatTokens(n)} summarized` : "context compacted";
  };

  return (
    <box paddingLeft={3} marginTop={1} flexDirection="row" gap={1}>
      <text fg={oc.textMuted}>⎯⎯</text>
      <text fg={oc.textMuted} wrapMode="none">
        {label()}
      </text>
    </box>
  );
}
