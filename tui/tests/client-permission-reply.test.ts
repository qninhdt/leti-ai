// Regression: the permission-reply request must match the server route
// exactly. The bug: client POSTed `/v1/permission/{id}/reply` with a
// `{reply,pattern,feedback}` body, but the server exposes `POST
// /v1/permission/{id}` taking `{decision,reason}`. The 404 left the ask
// unresolved, so the parked tool call hung forever ("Stale read" spinner)
// and no file was written. These lock the URL + body shape and the
// empty-200 response handling.

import { describe, expect, it } from "vitest";

import { createClient } from "../src/api/client.js";

import type { PermissionReplyDto } from "../src/api/types.js";

interface Captured {
  url: string;
  method: string;
  body: unknown;
}

// A fetch double that records the last call and returns a bare 200 OK with an
// EMPTY body — exactly what the server's permission route sends.
function recordingFetch(status = 200, responseBody = "") {
  const captured: Captured = { url: "", method: "", body: undefined };
  const fetch = async (input: string | URL, init?: RequestInit): Promise<Response> => {
    captured.url = String(input);
    captured.method = init?.method ?? "GET";
    captured.body = init?.body ? JSON.parse(init.body as string) : undefined;
    return new Response(responseBody, { status });
  };
  return { captured, fetch };
}

describe("client.replyPermission — server contract", () => {
  it("POSTs to /v1/permission/{askId} (no /reply suffix)", async () => {
    const { captured, fetch } = recordingFetch();
    const client = createClient({ baseUrl: "http://x", fetch });

    await client.replyPermission("ask-123", { decision: "allow" });

    expect(captured.method).toBe("POST");
    expect(captured.url).toBe("http://x/v1/permission/ask-123");
  });

  it("sends the {decision, reason} body shape the server expects", async () => {
    const { captured, fetch } = recordingFetch();
    const client = createClient({ baseUrl: "http://x", fetch });

    const body: PermissionReplyDto = { decision: "deny", reason: "nope" };
    await client.replyPermission("ask-9", body);

    expect(captured.body).toEqual({ decision: "deny", reason: "nope" });
  });

  it("does not throw on the route's empty 200 OK response", async () => {
    const { fetch } = recordingFetch(200, "");
    const client = createClient({ baseUrl: "http://x", fetch });

    // Would previously throw "Unexpected end of JSON input" on res.json().
    await expect(client.replyPermission("ask-1", { decision: "allow" })).resolves.toBeUndefined();
  });

  it("carries always_allow through verbatim", async () => {
    const { captured, fetch } = recordingFetch();
    const client = createClient({ baseUrl: "http://x", fetch });

    await client.replyPermission("ask-2", { decision: "always_allow" });

    expect(captured.body).toEqual({ decision: "always_allow" });
  });
});
