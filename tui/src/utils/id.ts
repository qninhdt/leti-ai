// crypto.randomUUID is Node 19+; fall back to a simple unique-enough ID
// for older runtimes. The id is consumed by the server's part validation
// and never leaves the request, so collision risk on fallback is
// acceptable.

export function randomId(): string {
  const g = globalThis as { crypto?: { randomUUID?: () => string } };
  if (g.crypto?.randomUUID) return g.crypto.randomUUID();
  return `tui-${Date.now()}-${Math.floor(Math.random() * 1e9)}`;
}
