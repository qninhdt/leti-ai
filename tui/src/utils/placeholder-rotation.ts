// Rotating placeholder text for the empty prompt, mirroring OpenCode's
// `Ask anything... "<example>"` line. OpenCode picks a random index per mount
// and per session change; we expose the example list plus a picker so the
// editor can do the same. Shell-mode placeholders are intentionally absent —
// shell mode was dropped from Leti.

export const PROMPT_PLACEHOLDERS: readonly string[] = [
  "Fix a TODO in the codebase",
  "What is the tech stack of this project?",
  "Fix broken tests",
  "Explain how the auth flow works",
  "Add a new API endpoint",
];

/// Pick a random index into a list of length `count`. Returns 0 for an empty
/// list so callers can index unconditionally.
export function randomPlaceholderIndex(count: number = PROMPT_PLACEHOLDERS.length): number {
  if (count <= 0) return 0;
  return Math.floor(Math.random() * count);
}

/// Render the placeholder line for a given rotation index, wrapping into the
/// `Ask anything... "<example>"` shape. Returns undefined when there are no
/// examples so the textarea shows no placeholder rather than empty quotes.
export function placeholderText(index: number): string | undefined {
  if (PROMPT_PLACEHOLDERS.length === 0) return undefined;
  const example = PROMPT_PLACEHOLDERS[index % PROMPT_PLACEHOLDERS.length];
  return `Ask anything... "${example}"`;
}
