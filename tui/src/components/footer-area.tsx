// Footer region below the message scrollbox. Renders the prompt editor for
// normal input. The permission/question footer prompt (shown when a turn is
// awaiting a reply) is filled in Phase 5; for now this always renders the
// editor. Kept as a thin switch point so Phase 5 can swap content without
// touching the routes that mount it.

import { PromptEditor } from "./prompt-editor.js";

export function FooterArea() {
  return <PromptEditor />;
}
