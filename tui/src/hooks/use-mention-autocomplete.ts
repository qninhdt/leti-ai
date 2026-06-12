// Mention autocomplete logic extracted from the prompt editor. Manages the
// @-mention query lifecycle: detecting the active mention under the cursor,
// fetching matching files with debounce, and accepting a selection.

import { createEffect, createSignal, on, onCleanup, type Accessor } from "solid-js";

import { activeMention } from "../utils/mention-parser.js";
import { useRuntime } from "../render/app-context.js";

import type { FileEntryDto } from "../api/types.js";
import type { TextareaRenderable } from "@opentui/core";

export interface MentionAutocomplete {
  mentionQuery: Accessor<string | null>;
  acFiles: Accessor<FileEntryDto[]>;
  acIndex: Accessor<number>;
  setAcIndex: (updater: number | ((prev: number) => number)) => void;
  popupOpen: () => boolean;
  acceptMention: (path: string) => void;
  refreshMention: () => void;
}

export function createMentionAutocomplete(
  getInput: () => TextareaRenderable | undefined,
): MentionAutocomplete {
  const runtime = useRuntime();
  const [mentionQuery, setMentionQuery] = createSignal<string | null>(null);
  const [acFiles, setAcFiles] = createSignal<FileEntryDto[]>([]);
  const [acIndex, setAcIndex] = createSignal(0);

  // Recompute the active @-mention from the textarea's buffer + cursor.
  function refreshMention(): void {
    const input = getInput();
    if (!input || input.isDestroyed) {
      setMentionQuery(null);
      return;
    }
    const span = activeMention(input.plainText, input.cursorOffset);
    setMentionQuery(span ? span.path : null);
    if (span) setAcIndex(0);
  }

  // Debounced listFiles fetch driven by the active mention query.
  createEffect(
    on(mentionQuery, (q) => {
      if (q === null) {
        setAcFiles([]);
        return;
      }
      const timer = setTimeout(() => {
        void runtime.client
          .listFiles(q)
          .then((res) => {
            if (mentionQuery() === q) setAcFiles(res.files);
          })
          .catch(() => setAcFiles([]));
      }, 120);
      onCleanup(() => clearTimeout(timer));
    }),
  );

  const popupOpen = () => mentionQuery() !== null && acFiles().length > 0;

  // Replace the active `@query` span with the chosen `@path` token.
  function acceptMention(path: string): void {
    const input = getInput();
    if (!input || input.isDestroyed) return;
    const buffer = input.plainText;
    const span = activeMention(buffer, input.cursorOffset);
    if (!span) return;
    const next = `${buffer.slice(0, span.start)}@${path} ${buffer.slice(span.end)}`;
    input.setText(next);
    input.cursorOffset = span.start + path.length + 2;
    setMentionQuery(null);
  }

  return {
    mentionQuery,
    acFiles,
    acIndex,
    setAcIndex,
    popupOpen,
    acceptMention,
    refreshMention,
  };
}
