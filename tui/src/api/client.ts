// Thin REST client. Once `npm run codegen` is run against a live server,
// replace this hand-rolled fetcher with openapi-fetch + paths from
// schema.d.ts. The interface stays stable so the store doesn't churn.

import type {
  AbortAckDto,
  AgentDto,
  CreateMessageDto,
  CreateSessionDto,
  FileContentDto,
  FileListDto,
  PermissionReplyDto,
  PluginInfoDto,
  PromptAckDto,
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
  abort(sessionId: string): Promise<AbortAckDto>;
  setMode(sessionId: string, body: SetModeDto): Promise<SessionDto>;
  replyPermission(askId: string, body: PermissionReplyDto): Promise<{ ok: true }>;
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
    if (res.status === 204) return undefined as T;
    return (await res.json()) as T;
  }

  return {
    health: () => request("GET", "/v1/health"),
    listAgents: () => request("GET", "/v1/agent"),
    listSessions: () => request("GET", "/v1/session"),
    createSession: (body) => request("POST", "/v1/session", body),
    getSession: (id) => request("GET", `/v1/session/${id}`),
    promptAsync: (id, body) => request("POST", `/v1/session/${id}/prompt_async`, body),
    abort: (id) => request("POST", `/v1/session/${id}/abort`),
    setMode: (id, body) => request("POST", `/v1/session/${id}/mode`, body),
    replyPermission: (askId, body) => request("POST", `/v1/permission/${askId}/reply`, body),
    listPlugins: () => request("GET", "/v1/plugin"),
    listFiles: (query) => request("GET", `/v1/files?query=${encodeURIComponent(query)}`),
    getFileContent: (path) => request("GET", `/v1/files/content?path=${encodeURIComponent(path)}`),
  };
}
