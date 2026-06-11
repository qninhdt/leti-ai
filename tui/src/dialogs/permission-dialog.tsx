// Permission dialog overlay, ported from the Ink `PermissionModal`. The 3-stage
// state machine (initial → always | deny_feedback) and its key triggers are
// preserved VERBATIM — this view governs tool authorization, so approval
// semantics, defaults, and scope handling are unchanged; only the layout moves
// to the OpenCode left-bar look. The full diff/bash body stays visible so the
// operator decides with complete information. The dialog owns its own Esc (the
// key router does not pop permission overlays), driving stage back-navigation;
// a resolved reply removes the overlay by askId.

import { createSignal, Show, on, createEffect, onCleanup, onMount } from "solid-js";
import { createPatch } from "diff";

import { theme } from "../theme/index.js";
import { useStore } from "../store/index.js";
import { useStoreSelector } from "../render/store-bridge.js";
import { useRuntime } from "../render/app-context.js";
import { setOverlayHandler } from "../render/key-router.js";
import { PROMPT_BODY_BORDER } from "../utils/border-chars.js";

import type { PermissionRequestDto } from "../api/types.js";
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
    request()?.patterns?.[0] ?? request()?.tool_name ?? "",
  );
  const [feedback, setFeedback] = createSignal("");

  // OverlayHost reuses ONE PermissionDialog instance across concurrent asks
  // (both permission entries match the same dispatch branch), so the signals
  // above seed only from the first askId. Re-seed them whenever askId changes
  // or a wrong-tool authorization could be sent: stage/pattern/feedback must
  // belong to the request currently being shown, not a prior one.
  createEffect(
    on(
      () => props.askId,
      () => {
        const req = useStore.getState().pendingPermissions[props.askId];
        setStage("initial");
        setPattern(req?.patterns?.[0] ?? req?.tool_name ?? "");
        setFeedback("");
      },
      { defer: true },
    ),
  );

  async function resolve(reply: {
    decision: "allow" | "deny" | "always";
    pattern?: string;
    feedback?: string;
  }): Promise<void> {
    useStore.getState().removeOverlay((e) => e.kind === "permission" && e.askId === props.askId);
    try {
      await runtime.client.replyPermission(props.askId, {
        reply: reply.decision,
        pattern: reply.pattern ?? null,
        feedback: reply.feedback ?? null,
      });
    } catch (err) {
      useStore.getState().setClientError(err instanceof Error ? err.message : String(err));
    }
  }

  // Printable char for the pattern/feedback editors: a single-char sequence at
  // or above space, excluding DEL — mirrors the engine's textarea filter.
  function printable(key: Parameters<KeyHandler>[0]): string | undefined {
    const seq = key.sequence;
    if (!seq || seq.length !== 1) return undefined;
    const code = seq.charCodeAt(0);
    if (code < 32 || code === 127) return undefined;
    return seq;
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
      else if (key.name === "return") void resolve({ decision: "always", pattern: pattern() });
      else if (key.name === "backspace" || key.name === "delete") setPattern((p) => p.slice(0, -1));
      else {
        const ch = printable(key);
        if (ch) setPattern((p) => p + ch);
      }
      return true;
    }
    // deny_feedback
    if (key.name === "escape") setStage("initial");
    else if (key.name === "return") void resolve({ decision: "deny", feedback: feedback() || undefined });
    else if (key.name === "backspace" || key.name === "delete") setFeedback((f) => f.slice(0, -1));
    else {
      const ch = printable(key);
      if (ch) setFeedback((f) => f + ch);
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

function diffColor(line: string): string {
  const oc = theme.oc;
  if (line.startsWith("+") && !line.startsWith("+++")) return oc.diffAdd;
  if (line.startsWith("-") && !line.startsWith("---")) return oc.diffDelete;
  return oc.textMuted;
}

function PermissionBody(props: { request: PermissionRequestDto }) {
  const oc = theme.oc;
  const diff = () => props.request.diff;
  const bash = () => props.request.bash;
  const patchLines = () => {
    const d = diff();
    if (!d) return [];
    return createPatch(d.filepath, d.before, d.after, "", "").split("\n").slice(4);
  };

  return (
    <Show
      when={diff()}
      fallback={
        <Show
          when={bash()}
          fallback={
            <box marginTop={1}>
              <text fg={oc.textMuted}>
                {props.request.tool_name}
                {props.request.reason ? ` — ${props.request.reason}` : ""}
              </text>
            </box>
          }
        >
          {(b) => (
            <box marginTop={1} flexDirection="column">
              <text fg={oc.text}>
                <span style={{ fg: oc.textMuted }}>$ </span>
                {b().command}
              </text>
              <text fg={oc.textMuted}>
                cwd: {b().cwd} · timeout: {b().timeout_ms}ms
              </text>
            </box>
          )}
        </Show>
      }
    >
      {(d) => (
        <box marginTop={1} flexDirection="column">
          <text fg={oc.textMuted}>{d().filepath}</text>
          {patchLines().map((line) => (
            <text fg={diffColor(line)} wrapMode="none">
              {line}
            </text>
          ))}
        </box>
      )}
    </Show>
  );
}
