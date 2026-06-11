// Format USD with 4-decimal precision (port of claw format_usd
// main.rs:3344-3357). Decimal-string in not arithmetic.

export function formatUsd(value: string | number | undefined | null): string {
  if (value === undefined || value === null) return "$0.0000";
  const n = typeof value === "string" ? Number.parseFloat(value) : value;
  if (Number.isNaN(n)) return "$0.0000";
  return `$${n.toFixed(4)}`;
}

export function shortId(id: string): string {
  return id.slice(0, 8);
}

// Compact token count the way OpenCode's prompt hint shows it: raw under 1000,
// otherwise one-decimal "k" (e.g. 1234 -> "1.2k"). Used in the editor hint row.
export function formatTokens(tokens: number): string {
  if (tokens < 1000) return String(tokens);
  return `${(tokens / 1000).toFixed(1)}k`;
}
