// Shared printable-character check used by dialogs that accept typed input
// (permission dialog, command palette). A printable char is a single-byte
// sequence at or above space (0x20), excluding DEL (0x7F).

export function isPrintable(code: number): boolean {
  return code >= 32 && code !== 127;
}
