// Full-screen engine mount wrapper. Mirrors OpenCode's bootstrap
// (cli/cmd/tui/app.tsx): build a CLI renderer, then hand a Solid component
// factory to @opentui/solid's render(). exitOnCtrlC is left enabled here so
// the spike proves clean terminal restore; later phases install a key router.

import { createCliRenderer, type CliRendererConfig } from "@opentui/core";
import { render } from "@opentui/solid";
import type { JSX } from "solid-js";

export interface MountOptions {
  exitOnCtrlC?: boolean;
  useMouse?: boolean;
}

function rendererConfig(opts: MountOptions): CliRendererConfig {
  return {
    targetFps: 60,
    gatherStats: false,
    exitOnCtrlC: opts.exitOnCtrlC ?? true,
    useMouse: opts.useMouse ?? true,
  };
}

/// Mounts a Solid component tree full-screen on a fresh CLI renderer.
export async function mount(root: () => JSX.Element, opts: MountOptions = {}): Promise<void> {
  const renderer = await createCliRenderer(rendererConfig(opts));
  await render(root, renderer);
}
