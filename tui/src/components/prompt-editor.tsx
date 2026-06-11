// The prompt editor, ported from OpenCode's `component/prompt/index.tsx` onto
// Openlet's store. A left-bar (`┃`) bordered multiline <textarea> with the meta
// row inside, the `╹`/`▀` shelf below, and the hint row under that. The
// textarea owns its own keys while focused (cursor movement, mid-string edit,
// wrapping) via the engine's focused-renderable dispatch; the global key router
// only handles overlays, and this editor blurs when one opens so there is one
// authoritative key path. Enter submits, Shift+Enter newlines (the engine's
// defaults are flipped via keyBindings), Up/Down walk prompt history, Tab is a
// Phase-6 autocomplete stub, and Esc arms the interrupt while a turn streams.

import { createEffect, createMemo, createSignal, on, onCleanup } from "solid-js";

import { theme } from "../theme/index.js";
import { useStore } from "../store/index.js";
import { useStoreSelector } from "../render/store-bridge.js";
import { useRuntime } from "../render/app-context.js";
import { createPromptSubmit } from "../render/use-prompt-submit.js";
import { PromptMetaRow } from "./prompt-meta-row.js";
import { PromptShelf } from "./prompt-shelf.js";
import { PromptHintRow } from "./prompt-hint-row.js";
import { PROMPT_BODY_BORDER } from "../utils/border-chars.js";
import { formatTokens, formatUsd } from "../utils/format.js";
import { placeholderText, randomPlaceholderIndex } from "../utils/placeholder-rotation.js";

import type { TextareaRenderable, KeyEvent, MouseEvent } from "@opentui/core";
import type { MessageView } from "../store/index.js";

const PALETTE_SHORTCUT = process.platform === "darwin" ? "⌘K" : "ctrl+k";
const INTERRUPT_RESET_MS = 5000;
const EMPTY_MESSAGES: MessageView[] = [];

// History flips Enter/Shift+Enter relative to the engine defaults (which are
// return->newline, meta+return->submit). Custom bindings override defaults.
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
  // On the home route (no session) show the agent /new would pick so the meta
  // row is populated; in a session show that session's agent.
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

  // Re-roll the placeholder when the session changes, like OpenCode.
  createEffect(on(activeSessionId, () => setPlaceholderIdx(randomPlaceholderIndex()), { defer: true }));

  // Disarm the interrupt whenever the turn stops streaming. Without this the
  // arm count survives into the next turn (one started by a slash-command or a
  // server retry, not this editor's Enter path), so a single Esc could abort
  // it — breaking the esc→esc-again→abort contract.
  createEffect(
    on(
      streaming,
      (s) => {
        if (!s) setInterruptCount(0);
      },
      { defer: true },
    ),
  );

  // Focus the textarea when no overlay is open; blur it while one is, so the
  // overlay (not the editor) owns the keyboard — matches OpenCode's dialog gate.
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
    // Down while not navigating is a no-op — there is nothing "below" the live
    // buffer to recall. Only Up enters history from the idle state.
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

  function onKeyDown(event: KeyEvent): void {
    if (event.name === "k" && event.ctrl) {
      // ⌘K / ctrl+k opens the command palette overlay; the editor blurs (the
      // overlay-open effect) so the palette owns the keyboard.
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
      // Autocomplete (@-files / commands) lands in Phase 6; swallow Tab so it
      // does not insert a literal tab into the prompt in the meantime.
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
            }}
            onKeyDown={onKeyDown}
            onSubmit={onSubmit}
            onMouseDown={(e: MouseEvent) => e.target?.focus()}
            ref={(r: TextareaRenderable) => {
              input = r;
            }}
          />
          <PromptMetaRow agent={agent()} model={model()} accent={accent()} />
        </box>
      </box>
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
