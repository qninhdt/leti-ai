// Key routing for the prompt editor's <textarea>. Layers three concerns in
// priority order: an open @-mention popup captures nav/accept/dismiss keys
// first, an open slash popup captures them next, and otherwise the base
// handler runs — command palette, prompt-history recall (Up/Down at buffer
// edges), Tab swallow, and Esc to arm the interrupt. Built as a factory so the
// editor injects its own closures (autocomplete hooks, input ref, history +
// interrupt callbacks) without this module reaching into component internals.

import { useStore } from "../store/index.js";

import type { MentionAutocomplete } from "../hooks/use-mention-autocomplete.js";
import type { SlashAutocomplete } from "../hooks/use-slash-autocomplete.js";
import type { KeyEvent, TextareaRenderable } from "@opentui/core";

export interface PromptKeyHandlerDeps {
  mention: MentionAutocomplete;
  slash: SlashAutocomplete;
  getInput: () => TextareaRenderable | undefined;
  recallHistory: (delta: -1 | 1) => void;
  armInterrupt: () => void;
}

export function createPromptKeyHandler(deps: PromptKeyHandlerDeps): (event: KeyEvent) => void {
  const { mention, slash, getInput, recallHistory, armInterrupt } = deps;

  function handleKey(event: KeyEvent): void {
    const input = getInput();
    if (event.name === "k" && event.ctrl) {
      event.preventDefault();
      useStore.getState().pushOverlay({ kind: "command_palette" });
      return;
    }
    if (event.name === "up" && (input?.cursorOffset ?? 0) === 0) {
      event.preventDefault();
      recallHistory(-1);
      return;
    }
    if (event.name === "down" && input && input.cursorOffset === input.plainText.length) {
      event.preventDefault();
      recallHistory(1);
      return;
    }
    if (event.name === "tab") {
      event.preventDefault();
      return;
    }
    if (event.name === "escape") {
      event.preventDefault();
      armInterrupt();
    }
  }

  return (event: KeyEvent): void => {
    if (mention.popupOpen()) {
      if (event.name === "up") {
        event.preventDefault();
        mention.setAcIndex((i: number) => Math.max(0, i - 1));
        return;
      }
      if (event.name === "down") {
        event.preventDefault();
        mention.setAcIndex((i: number) => Math.min(mention.acFiles().length - 1, i + 1));
        return;
      }
      if (event.name === "return" || event.name === "tab") {
        event.preventDefault();
        const choice = mention.acFiles()[mention.acIndex()];
        if (choice) mention.acceptMention(choice.path);
        return;
      }
      if (event.name === "escape") {
        event.preventDefault();
        mention.setAcIndex(0);
        return;
      }
    }
    if (slash.popupOpen()) {
      if (event.name === "up") {
        event.preventDefault();
        slash.setIndex((i: number) => Math.max(0, i - 1));
        return;
      }
      if (event.name === "down") {
        event.preventDefault();
        slash.setIndex((i: number) => Math.min(slash.suggestions().length - 1, i + 1));
        return;
      }
      if (event.name === "return") {
        // Run the highlighted command and clear the buffer — a slash prompt is
        // a command invocation, not text to submit to the model.
        event.preventDefault();
        slash.runSelection();
        return;
      }
      if (event.name === "tab") {
        event.preventDefault();
        slash.completeSelection();
        return;
      }
      if (event.name === "escape") {
        event.preventDefault();
        slash.dismiss();
        return;
      }
    }
    handleKey(event);
  };
}
