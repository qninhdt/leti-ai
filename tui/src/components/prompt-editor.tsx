// The prompt editor. A left-bar bordered multiline <textarea> with the meta
// row inside, the shelf below, and the hint row under that. The textarea owns
// its own keys while focused; the global key router only handles overlays.
// Enter submits, Shift+Enter newlines, Up/Down walk prompt history, and Esc
// arms the interrupt while a turn streams.

import { createEffect, createMemo, createSignal, on, onCleanup } from "solid-js";

import { theme } from "../theme/index.js";
import { useStore } from "../store/index.js";
import { useStoreSelector } from "../render/store-bridge.js";
import { useRuntime } from "../render/app-context.js";
import { createPromptSubmit } from "../render/use-prompt-submit.js";
import { createMentionAutocomplete } from "../hooks/use-mention-autocomplete.js";
import { createSlashAutocomplete } from "../hooks/use-slash-autocomplete.js";
import { PromptMetaRow } from "./prompt-meta-row.js";
import { ContextBar } from "./context-bar.js";
import { PromptShelf } from "./prompt-shelf.js";
import { PromptHintRow } from "./prompt-hint-row.js";
import { FileAutocomplete } from "../dialogs/file-autocomplete.js";
import { SlashAutocomplete } from "../dialogs/slash-autocomplete.js";
import { PROMPT_BODY_BORDER } from "../utils/border-chars.js";
import { formatTokens, formatUsd } from "../utils/format.js";
import { placeholderText, randomPlaceholderIndex } from "../utils/placeholder-rotation.js";

import { Show } from "solid-js";
import type { TextareaRenderable, KeyEvent, MouseEvent } from "@opentui/core";
import type { MessageView } from "../store/index.js";

const PALETTE_SHORTCUT = process.platform === "darwin" ? "⌘K" : "ctrl+k";
const INTERRUPT_RESET_MS = 5000;
const EMPTY_MESSAGES: MessageView[] = [];

const PROMPT_KEY_BINDINGS = [
  { name: "return", action: "submit" as const },
  { name: "kpenter", action: "submit" as const },
  { name: "linefeed", action: "submit" as const },
  { name: "return", shift: true, action: "newline" as const },
  { name: "kpenter", shift: true, action: "newline" as const },
];

export function PromptEditor() {
  const oc = theme.oc;
  const runtime = useRuntime();
  const submit = createPromptSubmit(runtime);

  let input: TextareaRenderable | undefined;
  const [value, setValue] = createSignal("");
  const [historyIdx, setHistoryIdx] = createSignal<number | null>(null);
  const [interruptCount, setInterruptCount] = createSignal(0);
  const [placeholderIdx, setPlaceholderIdx] = createSignal(randomPlaceholderIndex());
  let interruptTimer: ReturnType<typeof setTimeout> | undefined;

  const mention = createMentionAutocomplete(() => input);
  const slash = createSlashAutocomplete(() => input, runtime);

  const activeSessionId = useStoreSelector((s) => s.activeSessionId);
  const sessions = useStoreSelector((s) => s.sessions);
  const agents = useStoreSelector((s) => s.agents);
  const messages = useStoreSelector((s) => {
    const id = s.activeSessionId;
    return id ? s.messages[id] ?? EMPTY_MESSAGES : EMPTY_MESSAGES;
  });
  const overlayCount = useStoreSelector((s) => s.overlays.length);

  const session = createMemo(() => {
    const id = activeSessionId();
    return id ? sessions()[id] ?? null : null;
  });
  const agent = createMemo(() => {
    const s = session();
    const list = agents();
    return s ? list.find((a) => a.id === s.agent_id) ?? null : list[0] ?? null;
  });
  const model = createMemo(() => agent()?.model ?? "—");
  const accent = createMemo(() => (agent() ? oc.borderActive : oc.border));
  const streaming = createMemo(() => session()?.status === "running");

  const usageText = createMemo(() => {
    const list = messages();
    for (let i = list.length - 1; i >= 0; i--) {
      const total = list[i]?.step_finish?.usage_total;
      if (total && total > 0) return formatTokens(total);
    }
    return undefined;
  });
  const costText = createMemo(() => {
    const raw = session()?.cost_decimal_str;
    if (!raw) return undefined;
    return Number.parseFloat(raw) > 0 ? formatUsd(raw) : undefined;
  });

  // Latest provider-reported prompt tokens — the compaction anchor, so the
  // context bar tracks the same number should_compact does. Undefined before
  // the first turn returns usage (bar degrades to window size only).
  const contextTokens = createMemo(() => {
    const list = messages();
    for (let i = list.length - 1; i >= 0; i--) {
      const tok = list[i]?.step_finish?.context_tokens;
      if (tok && tok > 0) return tok;
    }
    return undefined;
  });

  createEffect(on(activeSessionId, () => setPlaceholderIdx(randomPlaceholderIndex()), { defer: true }));

  createEffect(
    on(
      streaming,
      (s) => {
        if (!s) setInterruptCount(0);
      },
      { defer: true },
    ),
  );

  createEffect(() => {
    if (!input || input.isDestroyed) return;
    if (overlayCount() > 0) {
      if (input.focused) input.blur();
    } else if (!input.focused) {
      input.focus();
    }
  });

  onCleanup(() => {
    if (interruptTimer) clearTimeout(interruptTimer);
  });

  function recallHistory(delta: -1 | 1): void {
    const list = runtime.history.list();
    if (list.length === 0) return;
    const current = historyIdx();
    if (current === null && delta > 0) return;
    const next = current === null ? list.length - 1 : current + delta;
    if (next < 0 || next >= list.length) {
      setHistoryIdx(null);
      if (delta > 0) applyText("");
      return;
    }
    setHistoryIdx(next);
    applyText(list[next]!);
  }

  function applyText(text: string): void {
    setValue(text);
    if (!input || input.isDestroyed) return;
    input.setText(text);
    input.gotoBufferEnd();
  }

  function armInterrupt(): void {
    const id = activeSessionId();
    if (!streaming() || !id) return;
    const next = interruptCount() + 1;
    setInterruptCount(next);
    if (interruptTimer) clearTimeout(interruptTimer);
    interruptTimer = setTimeout(() => setInterruptCount(0), INTERRUPT_RESET_MS);
    if (next >= 2) {
      void runtime.client.abort(id).catch(() => {});
      setInterruptCount(0);
    }
  }

  const onKeyDown = (event: KeyEvent): void => {
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

  function handleKey(event: KeyEvent): void {
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

  function onSubmit(): void {
    const text = input && !input.isDestroyed ? input.plainText : value();
    setHistoryIdx(null);
    setInterruptCount(0);
    applyText("");
    void submit(text);
  }

  return (
    <box flexShrink={0} flexDirection="column">
      <box border={["left"]} borderColor={accent()} customBorderChars={PROMPT_BODY_BORDER}>
        <box
          flexGrow={1}
          flexShrink={0}
          paddingLeft={2}
          paddingRight={2}
          paddingTop={1}
          backgroundColor={oc.backgroundElement}
        >
          <textarea
            placeholder={placeholderText(placeholderIdx())}
            placeholderColor={oc.textMuted}
            textColor={oc.text}
            focusedTextColor={oc.text}
            backgroundColor={oc.backgroundElement}
            focusedBackgroundColor={oc.backgroundElement}
            cursorColor={oc.text}
            minHeight={1}
            maxHeight={6}
            keyBindings={PROMPT_KEY_BINDINGS}
            onContentChange={() => {
              if (input && !input.isDestroyed) setValue(input.plainText);
              mention.refreshMention();
              slash.refresh();
            }}
            onCursorChange={() => mention.refreshMention()}
            onKeyDown={onKeyDown}
            onSubmit={onSubmit}
            onMouseDown={(e: MouseEvent) => e.target?.focus()}
            ref={(r: TextareaRenderable) => {
              input = r;
            }}
          />
          <PromptMetaRow
            agent={agent()}
            model={model()}
            accent={accent()}
            right={
              <Show when={agent()}>
                {(a) => (
                  <ContextBar
                    used={contextTokens()}
                    contextWindow={a().context_window ?? 0}
                    compactionThreshold={a().compaction_threshold ?? 0.8}
                  />
                )}
              </Show>
            }
          />
        </box>
      </box>
      <Show when={mention.popupOpen()}>
        <FileAutocomplete query={mention.mentionQuery() ?? ""} files={mention.acFiles()} index={mention.acIndex()} />
      </Show>
      <Show when={slash.popupOpen()}>
        <SlashAutocomplete query={slash.slashQuery() ?? ""} suggestions={slash.suggestions()} index={slash.index()} />
      </Show>
      <PromptShelf borderColor={accent()} />
      <PromptHintRow
        streaming={streaming()}
        interruptArmed={interruptCount() > 0}
        usage={usageText()}
        cost={costText()}
        paletteShortcut={PALETTE_SHORTCUT}
        spinnerColor={accent()}
      />
    </box>
  );
}
