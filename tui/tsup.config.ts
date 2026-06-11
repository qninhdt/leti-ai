import { defineConfig } from "tsup";
import { solidPlugin } from "esbuild-plugin-solid";

export default defineConfig({
  entry: ["src/cli.tsx"],
  format: ["esm"],
  target: "node20",
  outDir: "dist",
  outExtension: () => ({ js: ".mjs" }),
  clean: true,
  shims: true,
  banner: {
    // Engine (@opentui/core) needs native FFI -> requires the Bun runtime.
    // Node (even v22) rejects the FFI flags, so the CLI runs under Bun.
    js: "#!/usr/bin/env bun",
  },
  dts: false,
  minify: false,
  sourcemap: false,
  splitting: false,
  // Engine runs under Bun (native FFI). solid-js needs a compile-time JSX
  // transform (esbuild-plugin-solid) and a single CLIENT build instance:
  // bundle solid-js + @opentui/solid + opentui-spinner IN so the client
  // export conditions below apply and there is one shared solid instance.
  // @opentui/core/keymap carry native FFI -> keep external for Bun to load.
  noExternal: ["solid-js", "@opentui/solid", "opentui-spinner"],
  external: ["@opentui/core", "@opentui/keymap"],
  platform: "node",
  // @opentui/solid is a CUSTOM universal Solid renderer, not a DOM one.
  // generate:"dom" would emit solid-js/web calls hitting document.* (crashes
  // under Bun/TUI). generate:"universal" + moduleName routes the JSX runtime
  // calls to @opentui/solid's renderer instead.
  esbuildPlugins: [
    solidPlugin({ solid: { generate: "universal", moduleName: "@opentui/solid" } }),
  ],
  esbuildOptions(options) {
    // Lead with solid's universal-client `solid` condition so the bundled
    // solid-js is the DOM/client build (isServer=false), not the SSR build
    // that esbuild's node platform would otherwise select.
    options.conditions = ["solid", "browser", "import", "module", "default"];
  },
});
