// Assistant text part. Renders the part's text through the engine's rich
// `<markdown>` element at `paddingLeft=3 marginTop=1`, streaming-aware. While a
// part streams its finalized `text` is empty and the live tokens accumulate in
// `buffer`; we feed `text + buffer` so the markdown updates as deltas arrive.
//
// internalBlockMode="top-level" is load-bearing for smooth streaming: it keeps
// every top-level markdown block (paragraph, heading, list, fenced code) as its
// own child renderable, so appending a token only re-parses/re-highlights the
// trailing block while all prior blocks keep their exact renderable instances
// and are skipped by reference. The default "coalesced" mode merges the whole
// message into a single block that gets fully re-highlighted every token —
// which reads as flicker/lag on every keystroke of streamed text.

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
          internalBlockMode="top-level"
          conceal={true}
          concealCode={false}
          syntaxStyle={syntaxStyle}
          fg={oc.text}
          bg={oc.background}
        />
      </box>
    </Show>
  );
}
