// Merges server-authoritative message bodies (from GET /messages) into the
// store's streaming view. The SSE stream carries only part IDs — tool call
// name/args and tool results live in the DB and reach the UI ONLY through this
// hydration. Maps the server's tagged-union parts to the flat PartView the
// render consumes, folds each tool result into the assistant message that
// issued the matching call (so a block tool shows its output inline), and
// preserves an in-flight streaming message the server hasn't persisted yet.

import type { ServerMessageDto, ServerPartDto } from "../api/types.js";
import type { MessageView, PartView } from "./types.js";

type PartBase = Pick<PartView, "id" | "message_id" | "buffer" | "reasoning_buffer" | "status">;

function base(id: string): PartBase {
  return { id, message_id: "", buffer: "", reasoning_buffer: "", status: "complete" };
}

/// Map one server part to a PartView. Returns null for parts with no
/// inline-renderable body (step markers, compaction, attachments) so the
/// caller drops them.
export function serverPartToView(p: ServerPartDto): PartView | null {
  switch (p.kind) {
    case "text":
      return { ...base(p.id), kind: "text", text: p.text };
    case "reasoning":
      return { ...base(p.id), kind: "reasoning", text: p.text };
    case "tool_call":
      return {
        ...base(p.id),
        kind: "tool_call",
        tool_name: p.name,
        tool_args: p.args,
        tool_call_id: p.call_id,
      };
    case "tool_result": {
      const body = p.ok ? p.text ?? "" : p.error ?? "error";
      return {
        ...base(p.id),
        kind: "tool_result",
        tool_call_id: p.call_id,
        tool_result: body,
        status: p.ok ? "complete" : "errored",
      };
    }
    case "plan":
      return { ...base(p.id), kind: "text", text: p.plan };
    // step_start / step_finish / compaction / image / document carry no
    // inline body here — step_finish drives the footer via the step_finished
    // SSE event, images surface via attachment badges.
    default:
      return null;
  }
}

function mapMessage(m: ServerMessageDto): MessageView {
  const parts: PartView[] = [];
  for (const p of m.parts) {
    const view = serverPartToView(p);
    if (view) parts.push(view);
  }
  return { id: m.id, session_id: m.session_id, role: m.role, parts, created_at: m.created_at };
}

/// Move each tool result out of its `tool` role message and into the assistant
/// message that issued the matching call (inserted right after the call), so
/// the render's per-message result lookup folds it into the tool's card.
/// Emptied tool messages are dropped; orphan results (no matching call) keep a
/// standalone tool message so output is never lost.
function foldToolResults(messages: MessageView[]): MessageView[] {
  const callOwner = new Map<string, MessageView>();
  for (const m of messages) {
    if (m.role === "tool") continue;
    for (const part of m.parts) {
      if (part.kind === "tool_call" && part.tool_call_id) callOwner.set(part.tool_call_id, m);
    }
  }

  const out: MessageView[] = [];
  for (const m of messages) {
    if (m.role !== "tool") {
      out.push(m);
      continue;
    }
    const orphans: PartView[] = [];
    for (const part of m.parts) {
      const owner =
        part.kind === "tool_result" && part.tool_call_id
          ? callOwner.get(part.tool_call_id)
          : undefined;
      if (!owner) {
        orphans.push(part);
        continue;
      }
      const at = owner.parts.findIndex(
        (x) => x.kind === "tool_call" && x.tool_call_id === part.tool_call_id,
      );
      if (at >= 0) owner.parts.splice(at + 1, 0, part);
      else owner.parts.push(part);
    }
    if (orphans.length > 0) out.push({ ...m, parts: orphans });
  }
  return out;
}

// A message is still streaming while any part is accumulating live text —
// non-empty buffers, cleared by part_updated on finalize. A tool_call part
// sits at status "streaming" forever (no part_updated), so status alone is NOT
// a reliable streaming signal; buffer content is.
function isStreaming(m: MessageView): boolean {
  return m.parts.some((p) => p.buffer.length > 0 || p.reasoning_buffer.length > 0);
}

/// Produce the merged message list for a session: server-authoritative folded
/// messages, plus any store-only message the server hasn't persisted yet (the
/// in-flight streaming turn). A message that is present on both sides keeps its
/// store version when either (a) it is still streaming locally — so a
/// stale/empty server copy never clobbers live deltas — or (b) it is a user
/// message, whose optimistic store copy holds the CLEAN typed text + badge
/// chips, while the server copy carries the @mention-expanded text (embedded
/// file bodies) meant for the model, not the display.
export function hydrateMessages(
  existing: MessageView[],
  server: ServerMessageDto[],
): MessageView[] {
  const storeById = new Map(existing.map((m) => [m.id, m]));
  const folded = foldToolResults(server.map(mapMessage));

  const settled = folded.map((m) => {
    const store = storeById.get(m.id);
    if (store && (isStreaming(store) || store.role === "user")) return store;
    if (store?.badges) m.badges = store.badges;
    return m;
  });

  const serverIds = new Set(server.map((m) => m.id));
  const inflight = existing.filter((m) => !serverIds.has(m.id));
  return settled.concat(inflight);
}
