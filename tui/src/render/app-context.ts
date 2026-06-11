// Runtime dependencies shared across the Solid tree (REST client, prompt
// history, SSE connection params). Created once in cli.tsx and provided at the
// root so routes/footer/overlays can reach the client without prop-drilling.
// This is the Solid equivalent of the props the Ink <App> received.

import { createContext, useContext } from "solid-js";

import type { OpenletClient } from "../api/client.js";
import type { PromptHistory } from "../hooks/use-prompt-history.js";

export interface AppRuntime {
  client: OpenletClient;
  baseUrl: string;
  history: PromptHistory;
  token?: string;
  /// Quit the process (engine teardown + exit). Wired in cli.tsx mount.
  exit: () => void;
}

const AppRuntimeContext = createContext<AppRuntime>();

export const AppRuntimeProvider = AppRuntimeContext.Provider;

export function useRuntime(): AppRuntime {
  const ctx = useContext(AppRuntimeContext);
  if (!ctx) throw new Error("useRuntime called outside AppRuntimeProvider");
  return ctx;
}
