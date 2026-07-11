// The prompt editor. A left-bar bordered multiline <textarea> with the meta
// row inside, the shelf below, and the hint row under that. The textarea owns
// its own keys while focused; the global key router only handles overlays.
// Enter submits, Shift+Enter newlines, Up/Down walk prompt history, and Esc
// arms the interrupt while a turn streams.

import { createEffect, createSignal, on, onCleanup } from "solid-js";

import { theme } from "../theme/index.js";
import { useStoreSelector } from "../render/store-bridge.js";
import { useRuntime } from "../render/app-context.js";
import { createPromptSubmit } from "../render/use-prompt-submit.js";
import { createMentionAutocomplete } from "../hooks/use-mention-autocomplete.js";
import { createSlashAutocomplete } from "../hooks/use-slash-autocomplete.js";
import { createPromptDerived } from "./use-prompt-derived.js";
import { createInterruptArm } from "./use-interrupt-arm.js";
import { createPromptKeyHandler } from "./create-prompt-key-handler.js";
import { PromptMetaRow } from "./prompt-meta-row.js";
import { ContextBar } from "./context-bar.js";
import { PromptShelf } from "./prompt-shelf.js";
import { PromptHintRow } from "./prompt-hint-row.js";
import { FileAutocomplete } from "../dialogs/file-autocomplete.js";
import { SlashAutocomplete } from "../dialogs/slash-autocomplete.js";
import { PROMPT_BODY_BORDER } from "../utils/border-chars.js";
import { placeholderText, randomPlaceholderIndex } from "../utils/placeholder-rotation.js";

import { Show } from "solid-js";
import type { TextareaRenderable, KeyEvent, MouseEvent } from "@opentui/core";
import type { MessageView } from "../store/index.js";

const PALETTE_SHORTCUT = process.platform === "darwin" ? "⌘K" : "ctrl+k";
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
  const [placeholderIdx, setPlaceholderIdx] = createSignal(randomPlaceholderIndex());

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

  const { agent, model, accent, streaming, usageText, costText, contextTokens } =
    createPromptDerived({ activeSessionId, sessions, agents, messages, oc });

  const { interruptCount, armInterrupt, resetInterrupt } = createInterruptArm({
    activeSessionId,
    streaming,
    runtime,
  });

  createEffect(on(activeSessionId, () => setPlaceholderIdx(randomPlaceholderIndex()), { defer: true }));

  createEffect(() => {
    if (!input || input.isDestroyed) return;
    if (overlayCount() > 0) {
      if (input.focused) input.blur();
    } else if (!input.focused) {
      input.focus();
    }
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

  const onKeyDown = createPromptKeyHandler({
    mention,
    slash,
    getInput: () => input,
    recallHistory,
    armInterrupt,
  });

  function onSubmit(): void {
    const text = input && !input.isDestroyed ? input.plainText : value();
    setHistoryIdx(null);
    resetInterrupt();
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
