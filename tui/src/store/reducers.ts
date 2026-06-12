// Pure reducer helpers for immutable store updates. Each function produces a
// new messages map (or sub-structure) without mutating the input.

import type { MessageView, PartView } from "./types.js";

function emptyMessage(sessionId: string, messageId: string): MessageView {
  return {
    id: messageId,
    session_id: sessionId,
    role: "assistant",
    parts: [],
    created_at: new Date().toISOString(),
  };
}

export function getOrCreateMessage(
  list: MessageView[],
  sessionId: string,
  messageId: string,
): { list: MessageView[]; index: number } {
  const idx = list.findIndex((m) => m.id === messageId);
  if (idx >= 0) return { list, index: idx };
  const next = list.concat(emptyMessage(sessionId, messageId));
  return { list: next, index: next.length - 1 };
}

function upsertPart(parts: PartView[], partId: string): { parts: PartView[]; index: number } {
  const idx = parts.findIndex((p) => p.id === partId);
  if (idx >= 0) return { parts, index: idx };
  const part: PartView = {
    id: partId,
    message_id: "",
    kind: "text",
    text: "",
    buffer: "",
    reasoning_buffer: "",
    status: "streaming",
  };
  return { parts: parts.concat(part), index: parts.length };
}

// Immutably replace the message at `index` within a session's list and
// return the new top-level `messages` map.
export function withMessage(
  messages: Record<string, MessageView[]>,
  sessionId: string,
  list: MessageView[],
  index: number,
  message: MessageView,
): Record<string, MessageView[]> {
  const next = list.slice();
  next[index] = message;
  return { ...messages, [sessionId]: next };
}

// Ensure the message + part exist, then apply `update` to that part.
export function upsertPartInMessage(
  messages: Record<string, MessageView[]>,
  sessionId: string,
  messageId: string,
  partId: string,
  update: (part: PartView) => PartView,
): Record<string, MessageView[]> {
  const list = messages[sessionId] ?? [];
  const { list: withMsg, index: msgIdx } = getOrCreateMessage(list, sessionId, messageId);
  const msg = withMsg[msgIdx]!;
  const { parts, index: partIdx } = upsertPart(msg.parts, partId);
  const nextParts = parts.slice();
  nextParts[partIdx] = update(parts[partIdx]!);
  return withMessage(messages, sessionId, withMsg, msgIdx, { ...msg, parts: nextParts });
}

// Apply `update` to an existing message looked up by id. Returns null
// (no-op) when the message cannot be found.
export function updateMessageById(
  messages: Record<string, MessageView[]>,
  sessionId: string,
  messageId: string,
  update: (msg: MessageView) => MessageView | null,
): Record<string, MessageView[]> | null {
  const list = messages[sessionId] ?? [];
  const idx = list.findIndex((m) => m.id === messageId);
  if (idx < 0) return null;
  const updated = update(list[idx]!);
  return updated === null ? null : withMessage(messages, sessionId, list, idx, updated);
}

// Immutably replace a part within a message, looked up by id. Returns
// null when the part is absent so callers can treat it as a no-op.
export function updatePartById(
  msg: MessageView,
  partId: string,
  update: (part: PartView) => PartView,
): MessageView | null {
  const idx = msg.parts.findIndex((p) => p.id === partId);
  if (idx < 0) return null;
  const parts = msg.parts.slice();
  parts[idx] = update(parts[idx]!);
  return { ...msg, parts };
}
