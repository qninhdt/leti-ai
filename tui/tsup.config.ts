import { defineConfig } from "tsup";

export default defineConfig({
  entry: ["src/cli.tsx"],
  format: ["esm"],
  target: "node20",
  outDir: "dist",
  outExtension: () => ({ js: ".mjs" }),
  clean: true,
  shims: true,
  banner: {
    js: "#!/usr/bin/env node",
  },
  dts: false,
  minify: false,
  sourcemap: false,
  splitting: false,
});
