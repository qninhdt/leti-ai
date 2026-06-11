// File @-mention autocomplete popup — pure presentation. The prompt editor owns
// the lifecycle (it detects the active `@query`, debounces the listFiles fetch,
// tracks the selected index, and handles keys), because the textarea must keep
// focus so the user can refine the query as they type — a modal overlay would
// blur it. This component just renders the list + selection it is given,
// anchored inline above the prompt shelf.

import { For, Show } from "solid-js";

import { theme } from "../theme/index.js";

import type { FileEntryDto } from "../api/types.js";

export interface FileAutocompleteProps {
  query: string;
  files: FileEntryDto[];
  index: number;
}

export function FileAutocomplete(props: FileAutocompleteProps) {
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
      <text fg={oc.primary}>Files @{props.query}</text>
      <Show when={props.files.length > 0} fallback={<text fg={oc.textMuted}>(no matching files)</text>}>
        <For each={props.files}>
          {(file, i) => (
            <text fg={i() === props.index ? oc.accent : oc.text}>
              {i() === props.index ? "▸ " : "  "}
              {file.path}
            </text>
          )}
        </For>
      </Show>
      <text fg={oc.textMuted}>↑↓ select · Enter/Tab insert · Esc close</text>
    </box>
  );
}
