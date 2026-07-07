// Slash-command autocomplete for the prompt editor. Unlike @-mentions (async
// file lookups), slash commands resolve synchronously against the static
// command registry, so no debounce/fetch — just filter the registry as the
// user types. A slash command occupies the WHOLE prompt: the popup opens only
// when the buffer is exactly `/` followed by a command token (no spaces), so a
// literal "/" inside a normal sentence never triggers it.

import { createMemo, createSignal, type Accessor } from "solid-js";

import { complete, findCommand, type Suggestion } from "../commands/registry.js";
import { createCommandContext } from "../render/command-context.js";
import { useStore } from "../store/index.js";

import type { AppRuntime } from "../render/app-context.js";
import type { TextareaRenderable } from "@opentui/core";

// Match a bare slash-command prompt: leading `/`, then a single token of
// non-space chars (the query). A trailing space or any second token means the
// user has moved past command entry (e.g. a future `/cmd arg`), so the popup
// closes and Enter submits normally.
const SLASH_PROMPT = /^\/(\S*)$/;

export interface SlashAutocomplete {
  slashQuery: Accessor<string | null>;
  suggestions: Accessor<Suggestion[]>;
  index: Accessor<number>;
  setIndex: (updater: number | ((prev: number) => number)) => void;
  popupOpen: () => boolean;
  /// Close the popup without touching the buffer (Esc). It re-opens on the
  /// next keystroke if the buffer still matches a `/query`.
  dismiss: () => void;
  /// Complete the highlighted command name into the buffer (Tab), leaving the
  /// user to press Enter to run it.
  completeSelection: () => void;
  /// Run the highlighted command immediately (Enter) and clear the buffer.
  runSelection: () => void;
  refresh: () => void;
}

export function createSlashAutocomplete(
  getInput: () => TextareaRenderable | undefined,
  runtime: AppRuntime,
): SlashAutocomplete {
  const ctx = createCommandContext(runtime);
  const [slashQuery, setSlashQuery] = createSignal<string | null>(null);
  const [index, setIndex] = createSignal(0);

  const suggestions = createMemo(() => {
    const q = slashQuery();
    return q === null ? [] : complete(q);
  });

  function refresh(): void {
    const input = getInput();
    if (!input || input.isDestroyed) {
      setSlashQuery(null);
      return;
    }
    const m = SLASH_PROMPT.exec(input.plainText);
    if (m) {
      setSlashQuery(m[1] ?? "");
      setIndex(0);
    } else {
      setSlashQuery(null);
    }
  }

  const popupOpen = () => slashQuery() !== null && suggestions().length > 0;

  function dismiss(): void {
    setSlashQuery(null);
  }

  function clearInput(): void {
    const input = getInput();
    if (input && !input.isDestroyed) input.setText("");
    setSlashQuery(null);
  }

  function completeSelection(): void {
    const choice = suggestions()[index()];
    if (!choice) return;
    const input = getInput();
    if (!input || input.isDestroyed) return;
    input.setText(`${choice.display} `);
    input.gotoBufferEnd();
    setSlashQuery(null);
  }

  function runSelection(): void {
    const choice = suggestions()[index()];
    if (!choice) return;
    const cmd = findCommand(choice.display.replace(/^\//, ""));
    clearInput();
    if (!cmd) return;
    void Promise.resolve(cmd.run(ctx)).catch((err) => {
      useStore.getState().setClientError(err instanceof Error ? err.message : String(err));
    });
  }

  return {
    slashQuery,
    suggestions,
    index,
    setIndex,
    popupOpen,
    dismiss,
    completeSelection,
    runSelection,
    refresh,
  };
}
