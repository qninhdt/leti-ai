// Context-usage bar for the prompt meta row. Shows how full the model's
// context window is, anchored on the SAME number the backend's `should_compact`
// uses (the provider-reported prompt tokens, `usage.input_tokens`) so the bar
// and the actual auto-compaction trigger never disagree. Renders a small glyph
// bar + `<used> / <window>` label; color shifts muted → warning as usage nears
// the compaction threshold → error at/over it. Before the first turn returns
// usage (used == undefined) it degrades to the window size only, no bar, no NaN.

import { createMemo, Show } from "solid-js";

import { theme } from "../theme/index.js";
import { formatTokens } from "../utils/format.js";

// Bar width in cells. Small enough to sit inline in the meta row's right slot.
const BAR_CELLS = 8;
const FILLED = "█";
const EMPTY = "░";

export interface ContextBarProps {
  /// Provider-reported prompt tokens for the latest turn (the compaction
  /// anchor), or undefined before the first turn returns usage.
  used?: number;
  /// Total token budget of the model (denominator).
  contextWindow: number;
  /// Fraction of the window at which auto-compaction fires (0..1).
  compactionThreshold: number;
}

export function ContextBar(props: ContextBarProps) {
  const oc = theme.oc;

  // Fraction of the window used, clamped to [0,1]. Guard a zero/NaN window so
  // a misconfigured agent degrades to 0% instead of NaN%.
  const frac = createMemo(() => {
    const used = props.used;
    const win = props.contextWindow;
    if (used === undefined || !win || win <= 0) return undefined;
    return Math.max(0, Math.min(1, used / win));
  });

  // Color relative to the compaction threshold, not the hard limit: muted well
  // below, warning as it approaches, error at/over the point compaction fires.
  const color = () => {
    const f = frac();
    if (f === undefined) return oc.textMuted;
    const t = props.compactionThreshold > 0 ? props.compactionThreshold : 1;
    if (f >= t) return oc.error;
    if (f >= t * 0.85) return oc.warning;
    return oc.textMuted;
  };

  const bar = () => {
    const f = frac() ?? 0;
    const filled = Math.round(f * BAR_CELLS);
    return FILLED.repeat(filled) + EMPTY.repeat(Math.max(0, BAR_CELLS - filled));
  };

  const label = () => {
    const win = formatTokens(props.contextWindow);
    const used = props.used;
    // Before any usage, show the window size alone so the row isn't empty.
    // Once a turn reports usage, show used/window so the absolute token count
    // is visible, not just the fraction.
    return used === undefined || used <= 0 ? win : `${formatTokens(used)} / ${win}`;
  };

  return (
    <box flexDirection="row" gap={1} alignItems="center">
      <Show when={frac() !== undefined}>
        <text fg={color()} wrapMode="none">
          {bar()}
        </text>
      </Show>
      <text fg={oc.textMuted} wrapMode="none">
        {label()}
      </text>
    </box>
  );
}
