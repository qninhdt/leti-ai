// Assistant text part, ported from OpenCode's `TextPart`
// (`routes/session/index.tsx:1577`). Renders the part's text through the
// engine's rich `<markdown>` element (confirmed present in the Solid catalogue)
// at `paddingLeft=3 marginTop=1`, streaming-aware. While a part streams its
// finalized `text` is empty and the live tokens accumulate in `buffer`; we feed
// `text + buffer` so the markdown updates as deltas arrive.

import { Show } from "solid-js";

import { theme } from "../theme/index.js";
import { syntaxStyle } from "../theme/syntax-style.js";

import type { PartView } from "../store/index.js";

export interface PartTextProps {
  part: PartView;
}

export function PartText(props: PartTextProps) {
  const oc = theme.oc;
  const content = () => `${props.part.text ?? ""}${props.part.buffer}`.trim();

  return (
    <Show when={content()}>
      <box paddingLeft={3} marginTop={1} flexShrink={0}>
        <markdown
          content={content()}
          streaming={props.part.status !== "complete"}
          syntaxStyle={syntaxStyle}
          fg={oc.text}
          bg={oc.background}
        />
      </box>
    </Show>
  );
}
