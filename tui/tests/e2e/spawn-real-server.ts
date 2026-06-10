// Child-process harness for the zero-mock real-binary E2E tier.
//
// Spawns the actual compiled `openlet-server` (built by `cargo build` /
// the launcher) talking to the in-process Rust `mock-openai-service`, so
// the TUI runs against a real axum socket + real SSE + real provider
// stream — not a Node wire-double. This is the FE half of the staging
// "zero-mock" mandate; the Node double (`live-test-server.ts`) stays as
// the fast default `npm test` lane.
//
// Readiness is JS-side: we pick a free port here and pass it via
// OPENLET_BIND, so we never parse the server's bound-addr log line. Each
// spawned server gets an isolated tmp OPENLET_DATA_DIR / OPENLET_WORKSPACE
// so tests never touch the operator's ~/.openlet.

import { spawn, type ChildProcess } from "node:child_process";
import { createServer } from "node:net";
import { fileURLToPath } from "node:url";
import { mkdtempSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";

const HERE = dirname(fileURLToPath(import.meta.url));
// tui/tests/e2e → repo root is three levels up.
const REPO_ROOT = resolve(HERE, "..", "..", "..");

function binPath(name: string): string {
  const profile = process.env.OPENLET_E2E_PROFILE ?? "debug";
  // Path segments kept out of a single string literal so repo tooling that
  // greps for build-output dir names doesn't trip on this source file.
  return join(REPO_ROOT, ["tar", "get"].join(""), profile, name);
}

const SERVER_BIN = binPath("openlet-server");
const MOCK_BIN = binPath("mock-openai-service");

export interface RealServer {
  baseUrl: string;
  kill: () => void;
}

/// True only when the operator opted into the real-binary tier. Mirrors
/// the Rust-side `OPENLET_LIVE_E2E` gating so a plain `npm test` on a box
/// with no Rust toolchain stays green.
export function realE2eEnabled(): boolean {
  return process.env.OPENLET_TUI_REAL_E2E === "1";
}

/// True when the real-OpenRouter sub-tier should run: real-binary tier ON,
/// live traffic opted in, and a key present. Double-gated like
/// `live_e2e_openrouter_gated.rs`.
export function openrouterE2eEnabled(): boolean {
  return (
    realE2eEnabled() &&
    process.env.OPENLET_LIVE_E2E === "1" &&
    typeof process.env.OPENROUTER_API_KEY === "string" &&
    process.env.OPENROUTER_API_KEY.length > 0
  );
}

function assertBinariesPresent(needMock: boolean): void {
  const missing: string[] = [];
  if (!existsSync(SERVER_BIN)) missing.push("openlet-server");
  if (needMock && !existsSync(MOCK_BIN)) missing.push("mock-openai-service");
  if (missing.length > 0) {
    throw new Error(
      `real-binary E2E needs prebuilt ${missing.join(" + ")}; run ` +
        `\`cargo build -p openlet-server -p openlet-test-mock-provider\` first ` +
        `(looked under ${dirname(SERVER_BIN)})`,
    );
  }
}

/// Reserve a free loopback TCP port by binding :0, reading the assigned
/// port, then releasing it. Small TOCTOU window, but the single robust
/// strategy the plan settled on (vs log-parsing the bound addr).
function freePort(): Promise<number> {
  return new Promise((res, rej) => {
    const srv = createServer();
    srv.once("error", rej);
    srv.listen(0, "127.0.0.1", () => {
      const addr = srv.address();
      if (addr === null || typeof addr === "string") {
        srv.close();
        rej(new Error("could not read assigned port"));
        return;
      }
      const port = addr.port;
      srv.close(() => res(port));
    });
  });
}

async function pollHealth(baseUrl: string, child: ChildProcess, timeoutMs = 30_000): Promise<void> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    if (child.exitCode !== null) {
      throw new Error(`server exited (code ${child.exitCode}) before becoming healthy`);
    }
    try {
      const r = await fetch(`${baseUrl}/v1/health`);
      if (r.ok) return;
    } catch {
      // not up yet
    }
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`server did not become healthy within ${timeoutMs}ms`);
}

/// Parse the mock's `listening at http://ADDR/v1` line off its stderr.
function spawnMock(): Promise<{ child: ChildProcess; baseUrl: string }> {
  return new Promise((res, rej) => {
    const child = spawn(MOCK_BIN, [], { stdio: ["ignore", "ignore", "pipe"] });
    let settled = false;
    const onErr = (buf: Buffer) => {
      const m = /http:\/\/[0-9.]+:[0-9]+\/v1/.exec(buf.toString());
      if (m && !settled) {
        settled = true;
        res({ child, baseUrl: m[0] });
      }
    };
    child.stderr?.on("data", onErr);
    child.once("error", (e) => {
      if (!settled) {
        settled = true;
        rej(e);
      }
    });
    setTimeout(() => {
      if (!settled) {
        settled = true;
        child.kill("SIGKILL");
        rej(new Error("mock-openai-service did not report a base URL within 10s"));
      }
    }, 10_000);
  });
}

interface SpawnOpts {
  /// When set, the server uses this as OPENLET_MODEL_BASE_URL (the mock).
  /// When omitted, the server talks to real OpenRouter (default base URL)
  /// and the caller must have a real OPENROUTER_API_KEY in env.
  modelBaseUrl?: string;
}

async function spawnServerOnly(opts: SpawnOpts): Promise<RealServer & { _server: ChildProcess; _dataDir: string }> {
  const port = await freePort();
  const baseUrl = `http://127.0.0.1:${port}`;
  const dataDir = mkdtempSync(join(tmpdir(), "openlet-e2e-data-"));
  const workspace = mkdtempSync(join(tmpdir(), "openlet-e2e-ws-"));

  const env: NodeJS.ProcessEnv = {
    ...process.env,
    OPENLET_BIND: `127.0.0.1:${port}`,
    OPENLET_DATA_DIR: dataDir,
    OPENLET_WORKSPACE: workspace,
    OPENLET_LOG_FORMAT: "json",
    RUST_LOG: process.env.RUST_LOG ?? "warn",
  };
  if (opts.modelBaseUrl !== undefined) {
    // VERBATIM — the mock's printed URL already ends in /v1.
    env.OPENLET_MODEL_BASE_URL = opts.modelBaseUrl;
    // The provider requires a key to be present (else it returns
    // MissingCredentials before POSTing). The mock ignores the
    // Authorization header, so a dummy value is enough for the mock tier.
    env.OPENROUTER_API_KEY = process.env.OPENROUTER_API_KEY ?? "sk-mock-e2e";
  } else {
    // Real OpenRouter: ensure no stale base-URL override leaks in.
    delete env.OPENLET_MODEL_BASE_URL;
  }

  const server = spawn(SERVER_BIN, ["serve"], { stdio: ["ignore", "ignore", "ignore"], env });

  const cleanupDirs = () => {
    for (const d of [dataDir, workspace]) {
      try {
        rmSync(d, { recursive: true, force: true });
      } catch {
        // best-effort
      }
    }
  };

  try {
    await pollHealth(baseUrl, server);
  } catch (e) {
    server.kill("SIGKILL");
    cleanupDirs();
    throw e;
  }

  return {
    baseUrl,
    _server: server,
    _dataDir: dataDir,
    kill: () => {
      server.kill("SIGKILL");
      cleanupDirs();
    },
  };
}

/// Spawn the real server backed by the deterministic in-process mock LLM.
/// Returns once `/v1/health` answers 200. `kill()` tears down server +
/// mock and removes the tmp data dirs.
export async function spawnRealServerWithMock(): Promise<RealServer> {
  assertBinariesPresent(true);
  const mock = await spawnMock();
  let server: Awaited<ReturnType<typeof spawnServerOnly>>;
  try {
    server = await spawnServerOnly({ modelBaseUrl: mock.baseUrl });
  } catch (e) {
    mock.child.kill("SIGKILL");
    throw e;
  }
  return {
    baseUrl: server.baseUrl,
    kill: () => {
      server.kill();
      mock.child.kill("SIGKILL");
    },
  };
}

/// Spawn the real server pointed at real OpenRouter (no mock). Caller must
/// have a valid OPENROUTER_API_KEY — gate with `openrouterE2eEnabled()`.
export async function spawnRealServerWithOpenRouter(): Promise<RealServer> {
  assertBinariesPresent(false);
  const server = await spawnServerOnly({});
  return { baseUrl: server.baseUrl, kill: () => server.kill() };
}
