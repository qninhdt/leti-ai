// Drives message hydration: fetches server-authoritative bodies (GET /messages)
// and merges them into the store. The SSE stream delivers only part ids, so
// tool call name/args/results reach the UI exclusively through this fetch.
//
// Trigger policy (chosen to be correct + cheap, not live):
//   - on session activation — history shows tool calls immediately
//   - when a turn settles (session goes idle/errored/cancelled) — the just-run
//     tool calls + results are now durable, fetch their bodies
//   - on a `message_created` for the tool-result message (role unknown from the
//     event, so any mid-turn message_created schedules a coalesced refetch)
//
// Concurrent/rapid triggers for one session coalesce: an in-flight fetch sets a
// "dirty" flag that fires exactly one more fetch when it lands, so a burst of
// events never stacks requests.

import type { OpenletClient } from "../api/client.js";
import { useStore } from "../store/index.js";
import type { EventDto } from "../api/types.js";

export interface HydrationController {
  /// Fetch + merge now (coalesced). Safe to call redundantly.
  refresh(sessionId: string): void;
  /// Inspect an SSE event and refresh if it implies new durable content.
  onEvent(ev: EventDto): void;
}

const SETTLED: ReadonlySet<string> = new Set(["idle", "errored", "cancelled"]);

export function createHydrationController(client: OpenletClient): HydrationController {
  const inflight = new Set<string>();
  const dirty = new Set<string>();

  async function run(sessionId: string): Promise<void> {
    if (inflight.has(sessionId)) {
      dirty.add(sessionId);
      return;
    }
    inflight.add(sessionId);
    try {
      const messages = await client.listMessages(sessionId);
      useStore.getState().hydrateSession(sessionId, messages);
    } catch {
      // A failed hydrate is non-fatal — the streaming view still shows text.
      // Swallow so a transient 404/network error can't crash the SSE handler.
    } finally {
      inflight.delete(sessionId);
      if (dirty.delete(sessionId)) void run(sessionId);
    }
  }

  return {
    refresh(sessionId) {
      void run(sessionId);
    },
    onEvent(ev) {
      if (ev.kind === "session_status" && SETTLED.has(ev.status)) {
        void run(ev.session_id);
      } else if (ev.kind === "message_created") {
        // A tool-result message (Role::Tool) arrives as a bare message_created
        // with no role; refetch to pick up its parts. Coalescing keeps this
        // from being chatty during a multi-tool turn.
        void run(ev.session_id);
      } else if (ev.kind === "part_updated") {
        // Typed control-state transitions (notably compaction
        // pending→committed/failed) carry no body in SSE. Rehydrate so the
        // live timeline uses the same persisted provenance as cold reload.
        void run(ev.session_id);
      }
    },
  };
}
