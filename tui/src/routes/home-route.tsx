// Home route — shown when no session is active. Centered column with the
// openlet logo + the prompt, vertically centered. Mirrors OpenCode's home
// layout. contentWidth caps the prompt at maxWidth=75.

import { FooterArea } from "../components/footer-area.js";
import { Logo } from "../components/logo.js";

export function HomeRoute() {
  return (
    <box flexGrow={1} flexDirection="column" alignItems="center" paddingLeft={2} paddingRight={2}>
      <box flexGrow={1} />
      <box height={4} />
      <Logo />
      <box height={1} />
      <box width="100%" maxWidth={75}>
        <FooterArea />
      </box>
      <box flexGrow={1} />
    </box>
  );
}
