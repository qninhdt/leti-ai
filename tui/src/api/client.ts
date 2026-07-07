// Thin REST client. Once `npm run codegen` is run against a live server,
// replace this hand-rolled fetcher with openapi-fetch + paths from
// schema.d.ts. The interface stays stable so the store doesn't churn.

import type {
  AbortAckDto,
  AgentDto,
  CompactAckDto,
  CreateMessageDto,
  CreateSessionDto,
  FileContentDto,
  FileListDto,
  PermissionReplyDto,
  PluginInfoDto,
  PromptAckDto,
  QuestionAnswerDto,
  ServerMessageDto,
  SessionDto,
  SetModeDto,
} from "./types.js";

export type FetchFn = (input: string | URL, init?: RequestInit) => Promise<Response>;

export interface ClientConfig {
  baseUrl: string;
  token?: string;
  fetch?: FetchFn;
}

export class ApiError extends Error {
  constructor(
    public status: number,
    public code: string,
    message: string,
  ) {
    super(message);
  }
}

export interface OpenletClient {
  health(): Promise<{ ok: boolean }>;
  listAgents(): Promise<AgentDto[]>;
  listSessions(): Promise<SessionDto[]>;
  createSession(body: CreateSessionDto): Promise<SessionDto>;
  getSession(id: string): Promise<SessionDto>;
  promptAsync(sessionId: string, body: CreateMessageDto): Promise<PromptAckDto>;
  listMessages(sessionId: string): Promise<ServerMessageDto[]>;
  abort(sessionId: string): Promise<AbortAckDto>;
  compact(sessionId: string): Promise<CompactAckDto>;
  setMode(sessionId: string, body: SetModeDto): Promise<SessionDto>;
  replyPermission(askId: string, body: PermissionReplyDto): Promise<{ ok: true }>;
  answerQuestion(sessionId: string, body: QuestionAnswerDto): Promise<void>;
  listPlugins(): Promise<PluginInfoDto[]>;
  listFiles(query: string): Promise<FileListDto>;
  getFileContent(path: string): Promise<FileContentDto>;
}

export function createClient(config: ClientConfig): OpenletClient {
  const fetcher: FetchFn = config.fetch ?? ((input, init) => fetch(input, init));
  const headers = (): Record<string, string> => {
    const h: Record<string, string> = { "content-type": "application/json" };
    if (config.token) h.authorization = `Bearer ${config.token}`;
    return h;
  };
  const url = (p: string) => `${config.baseUrl.replace(/\/$/, "")}${p}`;

  async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
    const res = await fetcher(url(path), {
      method,
      headers: headers(),
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    if (!res.ok) {
      const text = await res.text().catch(() => "");
      let code = "http_error";
      let message = text || res.statusText;
      try {
        const parsed = JSON.parse(text) as { code?: string; message?: string };
        if (parsed.code) code = parsed.code;
        if (parsed.message) message = parsed.message;
      } catch {}
      throw new ApiError(res.status, code, message);
    }
    // 204, or any 2xx with an empty/no-JSON body (e.g. the permission-reply
    // route returns a bare 200 OK). Guard `res.json()` so those don't throw
    // "Unexpected end of JSON input" and get misreported as a client error.
    if (res.status === 204) return undefined as T;
    const raw = await res.text();
    if (raw.length === 0) return undefined as T;
    return JSON.parse(raw) as T;
  }

  return {
    health: () => request("GET", "/v1/health"),
    listAgents: () => request("GET", "/v1/agent"),
    listSessions: () => request("GET", "/v1/session"),
    createSession: (body) => request("POST", "/v1/session", body),
    getSession: (id) => request("GET", `/v1/session/${id}`),
    promptAsync: (id, body) => request("POST", `/v1/session/${id}/prompt_async`, body),
    listMessages: (id) => request("GET", `/v1/session/${id}/messages`),
    abort: (id) => request("POST", `/v1/session/${id}/abort`),
    compact: (id) => request("POST", `/v1/session/${id}/compact`),
    setMode: (id, body) => request("POST", `/v1/session/${id}/mode`, body),
    replyPermission: (askId, body) => request("POST", `/v1/permission/${askId}`, body),
    answerQuestion: (id, body) => request("POST", `/v1/session/${id}/question/answer`, body),
    listPlugins: () => request("GET", "/v1/plugin"),
    listFiles: (query) => request("GET", `/v1/files?query=${encodeURIComponent(query)}`),
    getFileContent: (path) => request("GET", `/v1/files/content?path=${encodeURIComponent(path)}`),
  };
}
