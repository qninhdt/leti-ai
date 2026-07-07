// Pure parsers for the `ask_user` tool's args + result. Split from the tsx
// renderer so they're unit-testable without Solid's JSX runtime. Args shape:
// `{ header, question, options:[{label, description?}], multi_select }`.
// Result shape: `{ selected:[idx], selected_labels:[str] }`.

export interface AskOption {
  label: string;
  description?: string;
}

export interface AskSpec {
  header: string;
  question: string;
  options: AskOption[];
  multi_select: boolean;
}

/// Extract the question spec from raw tool args. Tolerant: missing/malformed
/// fields degrade to empty strings / empty option list rather than throwing.
export function parseAskUser(args: unknown): AskSpec {
  const a = args && typeof args === "object" ? (args as Record<string, unknown>) : {};
  const header = typeof a.header === "string" ? a.header : "";
  const question = typeof a.question === "string" ? a.question : "";
  const multi_select = a.multi_select === true;
  const options: AskOption[] = [];
  if (Array.isArray(a.options)) {
    for (const o of a.options) {
      if (!o || typeof o !== "object") continue;
      const label = (o as { label?: unknown }).label;
      if (typeof label !== "string") continue;
      const description = (o as { description?: unknown }).description;
      options.push({
        label,
        description: typeof description === "string" ? description : undefined,
      });
    }
  }
  return { header, question, options, multi_select };
}

/// Pull the chosen labels out of the tool result. Prefers `selected_labels`
/// (the server echoes them for convenience); returns [] when unresolved.
export function selectedLabels(result: unknown): string[] {
  if (!result || typeof result !== "object") return [];
  const labels = (result as { selected_labels?: unknown }).selected_labels;
  if (!Array.isArray(labels)) return [];
  return labels.filter((l): l is string => typeof l === "string");
}
