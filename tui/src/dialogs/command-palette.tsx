// Command palette overlay (⌘K / ctrl+k). A prefix-filtered list over the
// existing slash-command registry — reuses `complete()` to list/filter and
// `findCommand()` to resolve, deliberately NOT a fuzzy-ranking subsystem (out of
// scope for the reskin). Typed printable keys build the query, Up/Down move the
// cursor, Enter runs the selected command via the shared command context, Esc
// falls through to the router's pop. Registers on the key router's overlay seam.

import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";

import { theme } from "../theme/index.js";
import { useStore } from "../store/index.js";
import { useRuntime } from "../render/app-context.js";
import { setOverlayHandler } from "../render/key-router.js";
import { createCommandContext } from "../render/command-context.js";
import { complete, findCommand } from "../commands/registry.js";

import type { KeyHandler } from "../render/key-router.js";

export function CommandPalette() {
  const oc = theme.oc;
  const runtime = useRuntime();
  const ctx = createCommandContext(runtime);

  const [query, setQuery] = createSignal("");
  const [index, setIndex] = createSignal(0);
  const suggestions = createMemo(() => complete(query()));

  function close(): void {
    useStore.getState().removeOverlay((e) => e.kind === "command_palette");
  }

  function run(): void {
    const list = suggestions();
    const choice = list[index()];
    if (!choice) return;
    const cmd = findCommand(choice.display.replace(/^\//, ""));
    close();
    if (cmd) void Promise.resolve(cmd.run(ctx)).catch((err) => {
      useStore.getState().setClientError(err instanceof Error ? err.message : String(err));
    });
  }

  const handler: KeyHandler = (key) => {
    if (key.name === "up") {
      setIndex((i) => Math.max(0, i - 1));
      return true;
    }
    if (key.name === "down") {
      setIndex((i) => Math.min(Math.max(0, suggestions().length - 1), i + 1));
      return true;
    }
    if (key.name === "return") {
      run();
      return true;
    }
    if (key.name === "backspace" || key.name === "delete") {
      setQuery((q) => q.slice(0, -1));
      setIndex(0);
      return true;
    }
    // Printable single char extends the query.
    const seq = key.sequence;
    if (seq && seq.length === 1) {
      const code = seq.charCodeAt(0);
      if (code >= 32 && code !== 127) {
        setQuery((q) => q + seq);
        setIndex(0);
        return true;
      }
    }
    return false;
  };

  onMount(() => setOverlayHandler(handler));
  onCleanup(() => setOverlayHandler(null));

  return (
    <box flexDirection="column" minWidth={48}>
      <text fg={oc.primary}>Commands</text>
      <text fg={oc.text}>
        <span style={{ fg: oc.textMuted }}>/ </span>
        {query()}
        <span style={{ fg: oc.borderActive }}>▌</span>
      </text>
      <Show when={suggestions().length > 0} fallback={<text fg={oc.textMuted}>(no matching commands)</text>}>
        <For each={suggestions()}>
          {(s, i) => (
            <box flexDirection="row">
              <text fg={i() === index() ? oc.accent : oc.text}>
                {i() === index() ? "▸ " : "  "}
                {s.display}
              </text>
              <text fg={oc.textMuted}> — {s.description}</text>
            </box>
          )}
        </For>
      </Show>
      <text fg={oc.textMuted}>↑↓ select · Enter run · Esc close</text>
    </box>
  );
}
