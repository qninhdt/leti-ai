// SyntaxStyle for the rich <markdown>/<code>/<diff> elements. These require a
// `syntaxStyle` prop; the renderer highlights BOTH prose and fenced code by
// feeding tree-sitter capture scopes through this style map.
//
// Why the old map rendered flat: the markdown renderable emits `markup.*`
// scopes (markup.heading.1..6, markup.strong, markup.italic, markup.raw,
// markup.link, markup.list, markup.quote, conceal, …) plus a mandatory
// `default` fallback — NONE of which the previous minimal map defined, so every
// heading/bold/link/inline-code collapsed to plain body text. Scope lookup
// falls back on dots ("markup.heading.1" → "markup.heading" → "markup") and
// finally to `default`, so `default` MUST be present.
//
// StyleDefinition supports foreground/background/bold/italic/underline/dim
// only (no native strikethrough — approximated with dim + muted color).
// Code-block highlighting resolves the tree-sitter singleton automatically; no
// treeSitterClient prop is needed for it to work.

import { SyntaxStyle, type ThemeTokenStyle } from "@opentui/core";

import { theme } from "../theme/index.js";

export function buildSyntaxStyle(): SyntaxStyle {
  const oc = theme.oc;
  const tokens: ThemeTokenStyle[] = [
    // Mandatory fallback — every unmatched scope resolves here.
    { scope: ["default"], style: { foreground: oc.text } },

    // --- Markdown prose (tree-sitter markdown grammar) ---
    // Headings: primary accent, bold; deeper levels shift to the cooler
    // accent so an outline reads at a glance.
    {
      scope: ["markup.heading", "markup.heading.1", "markup.heading.2"],
      style: { foreground: oc.primary, bold: true },
    },
    {
      scope: ["markup.heading.3", "markup.heading.4", "markup.heading.5", "markup.heading.6"],
      style: { foreground: oc.accent, bold: true },
    },
    { scope: ["markup.strong"], style: { foreground: oc.text, bold: true } },
    { scope: ["markup.italic"], style: { foreground: oc.text, italic: true } },
    { scope: ["markup.strikethrough"], style: { foreground: oc.textMuted, dim: true } },
    // Inline code + code-fence body text (before per-language highlight).
    { scope: ["markup.raw", "markup.raw.block"], style: { foreground: oc.success } },
    { scope: ["markup.link", "markup.link.url"], style: { foreground: oc.accent, underline: true } },
    { scope: ["markup.link.label"], style: { foreground: oc.accent } },
    {
      scope: ["markup.list", "markup.list.checked", "markup.list.unchecked"],
      style: { foreground: oc.secondary },
    },
    { scope: ["markup.quote"], style: { foreground: oc.textMuted, italic: true } },
    // Concealed syntax markers (#, **, backticks) + generic punctuation.
    {
      scope: ["conceal", "punctuation", "punctuation.delimiter", "punctuation.special"],
      style: { foreground: oc.textMuted },
    },
    { scope: ["label", "keyword.directive"], style: { foreground: oc.info } },
    { scope: ["string.escape", "character.special"], style: { foreground: oc.warning } },

    // --- Fenced code blocks (per-language tree-sitter scopes) ---
    { scope: ["comment"], style: { foreground: oc.textMuted, italic: true } },
    { scope: ["string", "symbol"], style: { foreground: oc.success } },
    { scope: ["number", "boolean", "constant"], style: { foreground: oc.warning } },
    { scope: ["keyword"], style: { foreground: oc.secondary, bold: true } },
    { scope: ["function", "function.call", "function.method"], style: { foreground: oc.accent } },
    { scope: ["type", "type.builtin", "constructor"], style: { foreground: oc.warning } },
    { scope: ["variable", "variable.parameter", "property"], style: { foreground: oc.text } },
    { scope: ["operator"], style: { foreground: oc.textMuted } },
    { scope: ["tag"], style: { foreground: oc.secondary } },
  ];
  return SyntaxStyle.fromTheme(tokens);
}

export const syntaxStyle = buildSyntaxStyle();
