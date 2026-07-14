import { For, Show, createEffect, createSignal, on, onCleanup, onMount } from "solid-js";

import { useRuntime } from "../render/app-context.js";
import { setRouteHandler } from "../render/key-router.js";
import { useStore } from "../store/index.js";
import { theme } from "../theme/index.js";
import { PROMPT_BODY_BORDER } from "../utils/border-chars.js";
import { isPrintable } from "../utils/printable.js";
import { PermissionBody } from "../dialogs/permission-body.js";
import {
  activatePermissionChoice,
  escapePermissionStage,
  initialPermissionFooterState,
  shiftPermissionChoice,
} from "./permission-footer-state.js";

import type { PermissionRequestDto, PermissionReplyKind } from "../api/types.js";
import type { KeyHandler } from "../render/key-router.js";

const CHOICES = ["Allow once", "Allow always", "Reject"] as const;
const ALWAYS_CHOICES = ["Confirm", "Cancel"] as const;

export function PermissionFooter(props: { request: PermissionRequestDto }) {
  const oc = theme.oc;
  const runtime = useRuntime();
  const [state, setState] = createSignal(initialPermissionFooterState());
  const [submitting, setSubmitting] = createSignal(false);

  createEffect(
    on(
      () => props.request.ask_id,
      () => {
        setState(initialPermissionFooterState());
        setSubmitting(false);
      },
      { defer: true },
    ),
  );

  async function reply(decision: PermissionReplyKind, reason?: string): Promise<void> {
    if (submitting()) return;
    const askId = props.request.ask_id;
    setSubmitting(true);
    try {
      await runtime.client.replyPermission(askId, { decision, reason: reason || null });
      // The authoritative permission_resolved SSE frame normally removes this
      // request. But the server treats a resolved reply as success even when
      // that event fails to publish (routes/permission.rs), so SSE may never
      // arrive. A successful POST is the fallback authority: apply the
      // resolution locally so the footer cannot lock forever. This is
      // idempotent with the SSE frame — permission_resolved deletes by ask_id,
      // and deleting an already-removed entry is a no-op.
      useStore.getState().applyEvent({ kind: "permission_resolved", ask_id: askId, decision });
    } catch (err) {
      setSubmitting(false);
      useStore.getState().setClientError(err instanceof Error ? err.message : String(err));
    }
  }

  function shift(delta: -1 | 1): void {
    setState((current) => shiftPermissionChoice(current, delta));
  }

  function activate(): void {
    const transition = activatePermissionChoice(state());
    setState(transition.state);
    if (transition.reply) void reply(transition.reply.decision, transition.reply.reason);
  }

  const handler: KeyHandler = (key) => {
    if (submitting()) return true;
    if (state().stage === "reject") {
      if (key.name === "escape") {
        setState((current) => escapePermissionStage(current));
      } else if (key.name === "return") activate();
      else if (key.name === "backspace" || key.name === "delete") {
        setState((current) => ({ ...current, feedback: current.feedback.slice(0, -1) }));
      }
      else if (key.sequence?.length === 1 && isPrintable(key.sequence.charCodeAt(0))) {
        setState((current) => ({ ...current, feedback: current.feedback + key.sequence }));
      }
      return true;
    }
    if (key.name === "escape") {
      setState((current) => escapePermissionStage(current));
    } else if (key.name === "left" || key.sequence === "h" || (key.name === "tab" && key.shift)) shift(-1);
    else if (key.name === "right" || key.sequence === "l" || key.name === "tab") shift(1);
    else if (key.name === "return") activate();
    return true;
  };

  onMount(() => setRouteHandler(handler));
  onCleanup(() => setRouteHandler(null));

  const choices = () => (state().stage === "always" ? ALWAYS_CHOICES : CHOICES);
  return (
    <box flexShrink={0} flexDirection="column" border={["left"]} borderColor={oc.warning} customBorderChars={PROMPT_BODY_BORDER}>
      <box backgroundColor={oc.backgroundElement} paddingTop={1} paddingBottom={1} paddingLeft={2} paddingRight={2} flexDirection="column">
        <text fg={oc.warning}>Permission required: {props.request.permission}</text>
        <PermissionBody request={props.request} />
        <Show when={state().stage === "reject"} fallback={
          <box marginTop={1} flexDirection="row" gap={1}>
            <For each={choices()}>{(choice, index) => {
              const active = () => state().selected === index();
              return <text bg={active() ? oc.warning : undefined} fg={active() ? oc.background : oc.textMuted}> {choice} </text>;
            }}</For>
          </box>
        }>
          <box marginTop={1} flexDirection="column">
            <text fg={oc.textMuted}>Optional rejection reason</text>
            <text fg={oc.text}>{state().feedback}<span style={{ fg: oc.borderActive }}>▌</span></text>
          </box>
        </Show>
        <text fg={oc.textMuted}>
          {submitting() ? "Submitting..." : state().stage === "reject" ? "Enter reject · Esc back" : "Tab/←→ select · Enter confirm · Esc reject"}
        </text>
      </box>
    </box>
  );
}
