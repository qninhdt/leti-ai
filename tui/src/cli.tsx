// Solid engine bootstrap. Builds the runtime deps (REST client, prompt history)
// and mounts <App> under the runtime provider on the @opentui/solid renderer.
// The full-screen shell, routing, overlays, bootstrap + SSE wiring live in App.

import { homedir } from "node:os";
import { join } from "node:path";

import { App } from "./app.js";
import { mount } from "./render/mount.js";
import { AppRuntimeProvider, type AppRuntime } from "./render/app-context.js";
import { createClient } from "./api/client.js";
import { PromptHistory } from "./services/prompt-history.js";

const DEFAULT_BASE_URL = process.env.LETI_BASE_URL ?? "http://127.0.0.1:8787";
const TOKEN = process.env.LETI_TOKEN;
const STATE_DIR = process.env.LETI_STATE_DIR ?? join(homedir(), ".leti");

const client = createClient({ baseUrl: DEFAULT_BASE_URL, token: TOKEN });
const history = new PromptHistory(join(STATE_DIR, "prompt-history.jsonl"));

const runtime: AppRuntime = {
  client,
  baseUrl: DEFAULT_BASE_URL,
  history,
  token: TOKEN,
  exit: () => process.exit(0),
};

void mount(() => (
  <AppRuntimeProvider value={runtime}>
    <App />
  </AppRuntimeProvider>
));
