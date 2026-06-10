// A real Node http server that speaks the exact openlet REST + SSE wire
// contract the TUI talks to. Used by the ink-testing-library E2E tests so
// the App runs against a genuine socket — real `fetch`, real `EventSource`,
// real reconnect/backoff — not a mocked client.
//
// This is deliberately NOT the Rust binary: the Rust server's own behavior
// is covered by the cargo `live_e2e_*` tests. Here we exercise the TUI's
// client + SSE + store + render pipeline against a faithful wire double, so
// `npm test` stays self-contained (no cargo build coupling).

import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { randomUUID } from "node:crypto";

export interface SeededAgent {
  id: string;
  // Mirror the real server's AgentDto, which emits `display_name` + `model`
  // (openlet_protocol::dto::AgentDto). The TUI reads `display_name`.
  display_name: string;
  description?: string | null;
  model?: string | null;
}

export interface SeededPlugin {
  id: string;
  name: string;
  version: string;
  capabilities: string[];
  enabled: boolean;
}

/// A single SSE frame the server will push to connected event clients.
export interface SseFrame {
  /// Dotted event name (e.g. "message.created") — matches routes/event.rs.
  event: string;
  /// JSON-serializable data payload.
  data: unknown;
  /// Optional durable event id (emitted as the SSE `id:` line).
  id?: number;
}

interface SseClient {
  res: ServerResponse;
  session?: string;
}

/// Live wire double. `start()` binds an ephemeral loopback port; `baseUrl`
/// is then ready for the TUI client + EventSource.
export class LiveTestServer {
  private server: Server;
  private sseClients: SseClient[] = [];
  private agents: SeededAgent[];
  private plugins: SeededPlugin[];
  private sessions = new Map<string, Record<string, unknown>>();
  private durableLog: SseFrame[] = [];
  public lastPrompt: { sessionId: string; body: unknown } | null = null;
  public lastPermissionReply: { askId: string; body: unknown } | null = null;
  public aborted: string[] = [];

  constructor(opts?: { agents?: SeededAgent[]; plugins?: SeededPlugin[] }) {
    this.agents = opts?.agents ?? [
      { id: randomUUID(), display_name: "general", description: "General agent", model: "mock/model-small" },
    ];
    this.plugins = opts?.plugins ?? [
      { id: "core-tools", name: "Core Tools", version: "0.1.0", capabilities: ["tools"], enabled: true },
    ];
    this.server = createServer((req, res) => void this.handle(req, res));
  }

  async start(): Promise<string> {
    await new Promise<void>((resolve) => this.server.listen(0, "127.0.0.1", resolve));
    const addr = this.server.address();
    if (addr === null || typeof addr === "string") throw new Error("no addr");
    return `http://127.0.0.1:${addr.port}`;
  }

  async stop(): Promise<void> {
    for (const c of this.sseClients) c.res.end();
    this.sseClients = [];
    await new Promise<void>((resolve, reject) =>
      this.server.close((err) => (err ? reject(err) : resolve())),
    );
  }

  agentId(): string {
    return this.agents[0]!.id;
  }

  /// Push an SSE frame to every connected event client whose session
  /// filter matches (or who subscribed globally). Durable frames (those
  /// with an `id`) are also appended to the replay log.
  push(frame: SseFrame): void {
    if (frame.id !== undefined) this.durableLog.push(frame);
    const payload = serializeFrame(frame);
    const sessionId = (frame.data as { session_id?: string } | undefined)?.session_id;
    for (const c of this.sseClients) {
      if (c.session && sessionId && c.session !== sessionId) continue;
      c.res.write(payload);
    }
  }

  /// Convenience: stream a complete assistant turn (message_created →
  /// part_created → N text deltas → part_updated → terminal idle status).
  pushAssistantTurn(sessionId: string, text: string, chunks = 2): void {
    const messageId = randomUUID();
    const partId = randomUUID();
    let id = this.durableLog.length + 1;
    this.push({ event: "message.created", id: id++, data: { session_id: sessionId, message_id: messageId, at: now() } });
    this.push({ event: "part.created", id: id++, data: { session_id: sessionId, message_id: messageId, part_id: partId, at: now() } });
    const slices = splitInto(text, chunks);
    for (const slice of slices) {
      this.push({
        event: "part.delta",
        data: { session_id: sessionId, message_id: messageId, part_id: partId, delta_kind: "text", delta: slice },
      });
    }
    this.push({ event: "part.updated", id: id++, data: { session_id: sessionId, message_id: messageId, part_id: partId, at: now() } });
    this.push({ event: "session.status", id: id++, data: { session_id: sessionId, status: "idle", at: now() } });
  }

  private async handle(req: IncomingMessage, res: ServerResponse): Promise<void> {
    const url = new URL(req.url ?? "/", "http://127.0.0.1");
    const path = url.pathname;
    const method = req.method ?? "GET";

    if (method === "GET" && path === "/v1/health") return json(res, 200, { ok: true });
    if (method === "GET" && path === "/v1/agent") return json(res, 200, this.agents);
    if (method === "GET" && path === "/v1/plugin") return json(res, 200, this.plugins);
    if (method === "GET" && path === "/v1/session") return json(res, 200, [...this.sessions.values()]);

    if (method === "POST" && path === "/v1/session") {
      const body = await readJson(req);
      const id = randomUUID();
      const session = {
        id,
        agent_id: (body as { agent_id?: string }).agent_id ?? this.agentId(),
        status: "idle",
        permission_mode: "read_only",
        created_at: now(),
        updated_at: now(),
        cost_decimal_str: "0.00",
      };
      this.sessions.set(id, session);
      return json(res, 201, session);
    }

    const sessionMatch = path.match(/^\/v1\/session\/([^/]+)$/);
    if (method === "GET" && sessionMatch) {
      const s = this.sessions.get(sessionMatch[1]!);
      return s ? json(res, 200, s) : json(res, 404, { code: "not_found", message: "no session" });
    }

    const promptMatch = path.match(/^\/v1\/session\/([^/]+)\/prompt_async$/);
    if (method === "POST" && promptMatch) {
      const sessionId = promptMatch[1]!;
      this.lastPrompt = { sessionId, body: await readJson(req) };
      return json(res, 202, { message_id: randomUUID() });
    }

    const abortMatch = path.match(/^\/v1\/session\/([^/]+)\/abort$/);
    if (method === "POST" && abortMatch) {
      this.aborted.push(abortMatch[1]!);
      return json(res, 200, { ok: true });
    }

    const permMatch = path.match(/^\/v1\/permission\/([^/]+)\/reply$/);
    if (method === "POST" && permMatch) {
      this.lastPermissionReply = { askId: permMatch[1]!, body: await readJson(req) };
      return json(res, 200, { ok: true });
    }

    if (method === "GET" && path === "/v1/event") {
      return this.openSse(req, res, url);
    }

    json(res, 404, { code: "not_found", message: path });
  }

  /// Number of currently-connected SSE clients. Tests wait on this to
  /// know the socket is truly established before pushing frames (frames
  /// pushed before a client connects with no Last-Event-ID are lost).
  clientCount(): number {
    return this.sseClients.length;
  }

  private openSse(req: IncomingMessage, res: ServerResponse, url: URL): void {
    res.writeHead(200, {
      "content-type": "text/event-stream",
      "cache-control": "no-cache",
      connection: "keep-alive",
    });
    // Flush headers + an initial comment so the client's EventSource sees
    // an open stream immediately (onopen), not only on the first event.
    res.write(":ok\n\n");
    const session = url.searchParams.get("session") ?? undefined;

    // Header-only Last-Event-ID resume — replay the durable tail past it.
    const lastId = req.headers["last-event-id"];
    if (typeof lastId === "string") {
      const after = parseInt(lastId, 10);
      if (!Number.isNaN(after)) {
        for (const frame of this.durableLog) {
          if (frame.id !== undefined && frame.id > after) res.write(serializeFrame(frame));
        }
      }
    }

    const client: SseClient = { res, session };
    this.sseClients.push(client);
    req.on("close", () => {
      this.sseClients = this.sseClients.filter((c) => c !== client);
    });
  }
}

function serializeFrame(frame: SseFrame): string {
  let out = "";
  if (frame.id !== undefined) out += `id:${frame.id}\n`;
  out += `event:${frame.event}\n`;
  out += `data:${JSON.stringify(frame.data)}\n\n`;
  return out;
}

function splitInto(text: string, n: number): string[] {
  if (n <= 1 || text.length <= 1) return [text];
  const size = Math.ceil(text.length / n);
  const out: string[] = [];
  for (let i = 0; i < text.length; i += size) out.push(text.slice(i, i + size));
  return out;
}

function now(): string {
  return new Date().toISOString();
}

async function readJson(req: IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = [];
  for await (const c of req) chunks.push(c as Buffer);
  const raw = Buffer.concat(chunks).toString("utf8");
  if (!raw) return {};
  try {
    return JSON.parse(raw);
  } catch {
    return {};
  }
}

function json(res: ServerResponse, status: number, body: unknown): void {
  const payload = JSON.stringify(body);
  res.writeHead(status, { "content-type": "application/json" });
  res.end(payload);
}
