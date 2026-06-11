// Minimal SyntaxStyle for the rich <markdown>/<code>/<diff> elements, which
// require a `syntaxStyle` prop. Built from the OpenCode palette via
// SyntaxStyle.fromTheme (the same constructor OpenCode's theme context uses).
// A fuller scope map (extmarks, per-token colors) lands with message rendering.

import { SyntaxStyle } from "@opentui/core";

import { theme } from "../theme/index.js";

export function buildSyntaxStyle(): SyntaxStyle {
  const oc = theme.oc;
  return SyntaxStyle.fromTheme([
    { scope: ["comment"], style: { foreground: oc.textMuted, italic: true } },
    { scope: ["string", "symbol"], style: { foreground: oc.success } },
    { scope: ["number", "boolean"], style: { foreground: oc.warning } },
    { scope: ["keyword"], style: { foreground: oc.secondary } },
    { scope: ["function", "function.call"], style: { foreground: oc.accent } },
    { scope: ["type"], style: { foreground: oc.warning } },
    { scope: ["variable"], style: { foreground: oc.text } },
    { scope: ["punctuation"], style: { foreground: oc.text } },
  ]);
}

export const syntaxStyle = buildSyntaxStyle();
