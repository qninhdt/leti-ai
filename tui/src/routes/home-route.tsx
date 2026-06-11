// Home route — shown when no session is active. Centered column with a logo
// placeholder and the prompt. Mirrors OpenCode's home: logo + prompt vertically
// centered, footer slot pinned bottom. The real logo lands in Phase 7; the real
// prompt editor in Phase 3. contentWidth caps the prompt at maxWidth=75.

import { FooterArea } from "../components/footer-area.js";
import { theme } from "../theme/index.js";

export function HomeRoute() {
  const oc = theme.oc;
  return (
    <box flexGrow={1} flexDirection="column" alignItems="center" paddingLeft={2} paddingRight={2}>
      <box flexGrow={1} />
      <box height={4} />
      <text fg={oc.primary}>openlet</text>
      <box height={1} />
      <box width="100%" maxWidth={75}>
        <FooterArea />
      </box>
      <box flexGrow={1} />
    </box>
  );
}
