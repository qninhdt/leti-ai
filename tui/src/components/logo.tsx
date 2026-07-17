// Leti home-route logo. OpenCode ships an ~880-line shimmer-animation logo
// engine; per the plan we keep ours small — a clean styled wordmark in the
// brand primary with a muted tagline. No animation (YAGNI); the visual anchor
// is the centered wordmark over the prompt, matching OpenCode's home layout.

import { theme } from "../theme/index.js";

export function Logo() {
  const oc = theme.oc;
  return (
    <box flexDirection="column" alignItems="center">
      <text fg={oc.primary} attributes={1}>
        leti
      </text>
      <text fg={oc.textMuted}>the open agent terminal</text>
    </box>
  );
}
