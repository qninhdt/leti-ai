// The `╹`/`▀` shelf that caps the prompt editor, ported from OpenCode's
// `component/prompt/index.tsx`. Two stacked 1-row boxes: the first continues
// the body's left rail with a `╹` terminus glyph, the second draws a `▀`
// half-block underline in the element background so the prompt has a grounded
// baseline. The border color matches the body rail (passed in by the editor).

import { theme } from "../theme/index.js";
import { PROMPT_SHELF_CAP_BORDER, PROMPT_SHELF_UNDERLINE_BORDER } from "../utils/border-chars.js";

export interface PromptShelfProps {
  /// Rail color — the editor lerps this between border and the agent accent.
  borderColor: string;
}

export function PromptShelf(props: PromptShelfProps) {
  const oc = theme.oc;
  return (
    <box
      height={1}
      border={["left"]}
      borderColor={props.borderColor}
      customBorderChars={PROMPT_SHELF_CAP_BORDER}
    >
      <box
        height={1}
        border={["bottom"]}
        borderColor={oc.backgroundElement}
        customBorderChars={PROMPT_SHELF_UNDERLINE_BORDER}
      />
    </box>
  );
}
