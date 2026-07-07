// Slash-command autocomplete popup — pure presentation, anchored inline above
// the prompt shelf. The prompt editor owns the lifecycle (detects the `/query`
// prompt, filters the registry, tracks the selected index, handles keys),
// because the textarea must keep focus so the user can refine the query as they
// type — a modal overlay would blur it. Mirrors FileAutocomplete for @-mentions.

import { For, Show } from "solid-js";

import { theme } from "../theme/index.js";

import type { Suggestion } from "../commands/registry.js";

export interface SlashAutocompleteProps {
  query: string;
  suggestions: Suggestion[];
  index: number;
}

export function SlashAutocomplete(props: SlashAutocompleteProps) {
  const oc = theme.oc;
  return (
    <box
      flexDirection="column"
      backgroundColor={oc.backgroundPanel}
      paddingLeft={2}
      paddingRight={2}
      marginLeft={2}
      minWidth={40}
    >
      <text fg={oc.primary}>Commands /{props.query}</text>
      <Show
        when={props.suggestions.length > 0}
        fallback={<text fg={oc.textMuted}>(no matching commands)</text>}
      >
        <For each={props.suggestions}>
          {(s, i) => (
            <box flexDirection="row">
              <text fg={i() === props.index ? oc.accent : oc.text}>
                {i() === props.index ? "▸ " : "  "}
                {s.display}
              </text>
              <text fg={oc.textMuted}> — {s.description}</text>
            </box>
          )}
        </For>
      </Show>
      <text fg={oc.textMuted}>↑↓ select · Enter run · Tab complete · Esc close</text>
    </box>
  );
}
