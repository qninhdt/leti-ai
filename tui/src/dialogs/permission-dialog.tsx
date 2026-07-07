// Permission dialog overlay. The 3-stage state machine (initial -> always |
// deny_feedback) and its key triggers are preserved — this view governs tool
// authorization, so approval semantics, defaults, and scope handling are
// unchanged. The dialog owns its own Esc driving stage back-navigation; a
// resolved reply removes the overlay by askId.

import { createSignal, Show, on, createEffect, onCleanup, onMount } from "solid-js";

import { theme } from "../theme/index.js";
import { useStore } from "../store/index.js";
import { useStoreSelector } from "../render/store-bridge.js";
import { useRuntime } from "../render/app-context.js";
import { setOverlayHandler } from "../render/key-router.js";
import { PROMPT_BODY_BORDER } from "../utils/border-chars.js";
import { isPrintable } from "../utils/printable.js";
import { PermissionBody } from "./permission-body.js";

import type { KeyHandler } from "../render/key-router.js";

type Stage = "initial" | "always" | "deny_feedback";

export interface PermissionDialogProps {
  askId: string;
}

export function PermissionDialog(props: PermissionDialogProps) {
  const oc = theme.oc;
  const runtime = useRuntime();
  const request = useStoreSelector((s) => s.pendingPermissions[props.askId]);

  const [stage, setStage] = createSignal<Stage>("initial");
  const [pattern, setPattern] = createSignal(
    request()?.patterns?.[0] ?? request()?.tool_name ?? request()?.permission ?? "",
  );
  const [feedback, setFeedback] = createSignal("");

  createEffect(
    on(
      () => props.askId,
      () => {
        const req = useStore.getState().pendingPermissions[props.askId];
        setStage("initial");
        setPattern(req?.patterns?.[0] ?? req?.tool_name ?? req?.permission ?? "");
        setFeedback("");
      },
      { defer: true },
    ),
  );

  async function resolve(reply: {
    decision: "allow" | "deny" | "always";
    feedback?: string;
  }): Promise<void> {
    useStore.getState().removeOverlay((e) => e.kind === "permission" && e.askId === props.askId);
    // Map the dialog's 3-stage vocabulary onto the server's PermissionReplyKind.
    // The "always" stage in this UI is always an allow (no always-deny path),
    // and the server derives the persisted rule pattern from the original ask,
    // so no pattern is sent. `feedback` (deny reason) rides along as `reason`.
    const decision =
      reply.decision === "always" ? "always_allow" : reply.decision;
    try {
      await runtime.client.replyPermission(props.askId, {
        decision,
        reason: reply.feedback ?? null,
      });
    } catch (err) {
      useStore.getState().setClientError(err instanceof Error ? err.message : String(err));
    }
  }

  const handler: KeyHandler = (key) => {
    const s = stage();
    if (s === "initial") {
      if (key.sequence === "1" || key.sequence === "a") void resolve({ decision: "allow" });
      else if (key.sequence === "2" || key.sequence === "A") setStage("always");
      else if (key.sequence === "3" || key.sequence === "r" || key.name === "escape")
        void resolve({ decision: "deny" });
      return true;
    }
    if (s === "always") {
      if (key.name === "escape") setStage("initial");
      else if (key.name === "return") void resolve({ decision: "always" });
      else if (key.name === "backspace" || key.name === "delete") setPattern((p) => p.slice(0, -1));
      else {
        const seq = key.sequence;
        if (seq && seq.length === 1 && isPrintable(seq.charCodeAt(0))) {
          setPattern((p) => p + seq);
        }
      }
      return true;
    }
    // deny_feedback
    if (key.name === "escape") setStage("initial");
    else if (key.name === "return") void resolve({ decision: "deny", feedback: feedback() || undefined });
    else if (key.name === "backspace" || key.name === "delete") setFeedback((f) => f.slice(0, -1));
    else {
      const seq = key.sequence;
      if (seq && seq.length === 1 && isPrintable(seq.charCodeAt(0))) {
        setFeedback((f) => f + seq);
      }
    }
    return true;
  };

  onMount(() => setOverlayHandler(handler));
  onCleanup(() => setOverlayHandler(null));

  return (
    <Show when={request()} fallback={<text fg={oc.textMuted}>resolved</text>}>
      {(req) => (
        <box
          border={["left"]}
          customBorderChars={PROMPT_BODY_BORDER}
          borderColor={oc.warning}
          backgroundColor={oc.backgroundPanel}
          paddingTop={1}
          paddingBottom={1}
          paddingLeft={2}
          paddingRight={2}
          flexDirection="column"
          minWidth={48}
        >
          <text fg={oc.warning}>Permission required: {req().permission}</text>
          <PermissionBody request={req()} />
          <Show when={stage() === "initial"}>
            <box marginTop={1} flexDirection="row" gap={2}>
              <text fg={oc.accent}>
                [1/a] <span style={{ fg: oc.text }}>allow once</span>
              </text>
              <text fg={oc.accent}>
                [2/A] <span style={{ fg: oc.text }}>always</span>
              </text>
              <text fg={oc.error}>
                [3/r/Esc] <span style={{ fg: oc.text }}>deny</span>
              </text>
            </box>
          </Show>
          <Show when={stage() === "always"}>
            <box marginTop={1} flexDirection="column">
              <text fg={oc.textMuted}>Edit pattern (Enter confirm, Esc back):</text>
              <text fg={oc.text}>
                {pattern()}
                <span style={{ fg: oc.borderActive }}>▌</span>
              </text>
            </box>
          </Show>
          <Show when={stage() === "deny_feedback"}>
            <box marginTop={1} flexDirection="column">
              <text fg={oc.textMuted}>Feedback (Enter confirm, Esc back):</text>
              <text fg={oc.text}>
                {feedback()}
                <span style={{ fg: oc.borderActive }}>▌</span>
              </text>
            </box>
          </Show>
        </box>
      )}
    </Show>
  );
}
