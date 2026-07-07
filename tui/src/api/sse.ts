// SSE wrapper around eventsource@3 — header-only Last-Event-ID resume,
// exponential reconnect backoff, single
// applyEvent dispatch on every frame. The dotted event names from
// `event_kind()` in routes/event.rs translate to snake_case on the
// EventDto wire shape (see api/types.ts).

import { EventSource } from "eventsource";

import type { EventDto, EventName } from "./types.js";

export type ConnState = "idle" | "connecting" | "open" | "reconnecting" | "error";

export interface SseConfig {
  baseUrl: string;
  sessionId?: string;
  token?: string;
  onEvent: (event: EventDto) => void;
  onState: (state: ConnState, detail?: { lastEventId?: number; attempt?: number }) => void;
}

const BACKOFF_MS = [250, 500, 1000, 2000, 5000] as const;

const KIND_FROM_NAME: Record<EventName, EventDto["kind"]> = {
  "session.status": "session_status",
  "message.created": "message_created",
  "part.created": "part_created",
  "part.delta": "part_delta",
  "part.updated": "part_updated",
  "step.finished": "step_finished",
  "permission.asked": "permission_asked",
  "permission.resolved": "permission_resolved",
  "question.requested": "question_requested",
  "plan_mode.entered": "plan_mode_entered",
  "plan_mode.exited": "plan_mode_exited",
  "error": "error",
  "heartbeat": "heartbeat",
  "plugin.error": "plugin_error",
};

const EVENT_NAMES: EventName[] = Object.keys(KIND_FROM_NAME) as EventName[];

export interface SseHandle {
  close(): void;
}

export function connectSse(config: SseConfig): SseHandle {
  let attempt = 0;
  let lastEventId: number | null = null;
  let source: EventSource | null = null;
  let closed = false;
  let backoffTimer: NodeJS.Timeout | null = null;

  const url = () => {
    const base = config.baseUrl.replace(/\/$/, "");
    const qs = config.sessionId ? `?session=${encodeURIComponent(config.sessionId)}` : "";
    return `${base}/v1/event${qs}`;
  };

  const open = () => {
    if (closed) return;
    config.onState(attempt === 0 ? "connecting" : "reconnecting", { attempt, lastEventId: lastEventId ?? undefined });

    const headers: Record<string, string> = {};
    if (config.token) headers.authorization = `Bearer ${config.token}`;
    if (lastEventId !== null) headers["Last-Event-ID"] = String(lastEventId);

    source = new EventSource(url(), {
      fetch: (input, init) =>
        fetch(input, { ...init, headers: { ...(init?.headers as Record<string, string>), ...headers } }),
    });

    source.onopen = () => {
      attempt = 0;
      config.onState("open", { lastEventId: lastEventId ?? undefined });
    };

    source.onerror = () => {
      if (closed) return;
      source?.close();
      source = null;
      const delay = BACKOFF_MS[Math.min(attempt, BACKOFF_MS.length - 1)] ?? 5000;
      attempt += 1;
      config.onState("reconnecting", { attempt, lastEventId: lastEventId ?? undefined });
      backoffTimer = setTimeout(open, delay);
    };

    for (const name of EVENT_NAMES) {
      source.addEventListener(name, (raw: MessageEvent) => {
        try {
          const data = raw.data ? (JSON.parse(raw.data) as Record<string, unknown>) : {};
          const kind = KIND_FROM_NAME[name];
          const ev = { kind, ...data } as EventDto;
          if (raw.lastEventId) {
            const id = parseInt(raw.lastEventId, 10);
            if (!Number.isNaN(id)) lastEventId = id;
          }
          config.onEvent(ev);
        } catch {
          // Skip malformed frames; bad SSE must never crash the client.
        }
      });
    }
  };

  open();

  return {
    close() {
      closed = true;
      if (backoffTimer) clearTimeout(backoffTimer);
      source?.close();
      source = null;
      config.onState("idle");
    },
  };
}
