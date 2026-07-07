// Question dialog overlay — renders a pending `ask_user` question and routes
// the user's selection back through POST /v1/session/:id/question/answer. The
// server suspends the tool (and the whole turn) until this resolves, so the
// dialog is the only way to unblock a session that is "working…" on a question.
//
// Keyboard: ↑/↓ move the cursor; Space toggles an option in multi-select;
// Enter confirms (single-select confirms the highlighted option directly).
// The dialog owns its keys via the router's overlay seam, mirroring the
// permission dialog. Esc is deliberately swallowed WITHOUT answering: the
// tool has a server-side timeout, and closing the prompt without a choice
// would just re-strand the turn — so the only exits are a real answer.

import { createSignal, For, Show, on, createEffect, onCleanup, onMount } from "solid-js";

import { theme } from "../theme/index.js";
import { useStore } from "../store/index.js";
import { useStoreSelector } from "../render/store-bridge.js";
import { useRuntime } from "../render/app-context.js";
import { setOverlayHandler } from "../render/key-router.js";
import { PROMPT_BODY_BORDER } from "../utils/border-chars.js";

import type { KeyHandler } from "../render/key-router.js";

export interface QuestionDialogProps {
  questionId: string;
}

export function QuestionDialog(props: QuestionDialogProps) {
  const oc = theme.oc;
  const runtime = useRuntime();
  const question = useStoreSelector((s) => s.pendingQuestions[props.questionId]);

  const [cursor, setCursor] = createSignal(0);
  // Selected option indices — only meaningful for multi-select; single-select
  // confirms the cursor row directly on Enter.
  const [checked, setChecked] = createSignal<Set<number>>(new Set());

  // Reset local state whenever the dialog is pointed at a different question.
  createEffect(
    on(
      () => props.questionId,
      () => {
        setCursor(0);
        setChecked(new Set<number>());
      },
      { defer: true },
    ),
  );

  async function submit(selected: number[]): Promise<void> {
    const q = useStore.getState().pendingQuestions[props.questionId];
    // Optimistically dismiss so the UI never double-fires an answer.
    useStore.getState().clearQuestion(props.questionId);
    if (!q) return;
    try {
      await runtime.client.answerQuestion(q.session_id, {
        question_id: props.questionId,
        selected,
      });
    } catch (err) {
      useStore.getState().setClientError(err instanceof Error ? err.message : String(err));
    }
  }

  const toggle = (i: number): void => {
    setChecked((prev) => {
      const next = new Set(prev);
      if (next.has(i)) next.delete(i);
      else next.add(i);
      return next;
    });
  };

  const handler: KeyHandler = (key) => {
    const q = useStore.getState().pendingQuestions[props.questionId];
    if (!q) return true;
    const count = q.options.length;
    if (key.name === "up") {
      setCursor((c) => Math.max(0, c - 1));
      return true;
    }
    if (key.name === "down") {
      setCursor((c) => Math.min(count - 1, c + 1));
      return true;
    }
    if (q.multi_select && (key.name === "space" || key.sequence === " ")) {
      toggle(cursor());
      return true;
    }
    if (key.name === "return") {
      if (q.multi_select) {
        // Multi-select confirms whatever is checked (may be empty — the
        // server accepts a zero-length selection for multi).
        void submit([...checked()].sort((a, b) => a - b));
      } else {
        void submit([cursor()]);
      }
      return true;
    }
    // Swallow everything else (incl. Esc) so stray keys don't leak to the
    // prompt editor behind the modal.
    return true;
  };

  onMount(() => setOverlayHandler(handler));
  onCleanup(() => setOverlayHandler(null));

  return (
    <Show when={question()} fallback={<text fg={oc.textMuted}>resolved</text>}>
      {(q) => (
        <box
          border={["left"]}
          customBorderChars={PROMPT_BODY_BORDER}
          borderColor={oc.accent}
          backgroundColor={oc.backgroundPanel}
          paddingTop={1}
          paddingBottom={1}
          paddingLeft={2}
          paddingRight={2}
          flexDirection="column"
          minWidth={52}
        >
          <text fg={oc.accent}>{q().header}</text>
          <box marginTop={1}>
            <text fg={oc.text}>{q().question}</text>
          </box>
          <box marginTop={1} flexDirection="column">
            <For each={q().options}>
              {(opt, i) => {
                const active = () => cursor() === i();
                const on = () => checked().has(i());
                const marker = () =>
                  q().multi_select ? (on() ? "[x]" : "[ ]") : active() ? "›" : " ";
                return (
                  <box flexDirection="row" gap={1}>
                    <text fg={active() ? oc.accent : oc.textMuted}>{marker()}</text>
                    <text fg={active() ? oc.text : oc.textMuted} wrapMode="none">
                      {opt.label}
                      {opt.description ? ` — ${opt.description}` : ""}
                    </text>
                  </box>
                );
              }}
            </For>
          </box>
          <box marginTop={1}>
            <text fg={oc.textMuted}>
              {q().multi_select
                ? "↑/↓ move · Space toggle · Enter confirm"
                : "↑/↓ move · Enter select"}
            </text>
          </box>
        </box>
      )}
    </Show>
  );
}
