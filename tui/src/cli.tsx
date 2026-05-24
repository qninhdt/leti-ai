import React from "react";
import { render } from "ink";
import { homedir } from "node:os";
import { join } from "node:path";

import { App } from "./app.js";
import { createClient } from "./api/client.js";
import { PromptHistory } from "./hooks/use-prompt-history.js";

const DEFAULT_BASE_URL = process.env.OPENLET_BASE_URL ?? "http://127.0.0.1:8787";
const TOKEN = process.env.OPENLET_TOKEN;
const STATE_DIR = process.env.OPENLET_STATE_DIR ?? join(homedir(), ".openlet");

function main(): void {
  const client = createClient({ baseUrl: DEFAULT_BASE_URL, token: TOKEN });
  const history = new PromptHistory(join(STATE_DIR, "prompt-history.jsonl"));
  render(<App client={client} baseUrl={DEFAULT_BASE_URL} history={history} token={TOKEN} />);
}

main();
