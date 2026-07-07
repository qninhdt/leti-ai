// Shared session lifecycle actions. Consolidates the duplicated
// "create session + push to store + activate" pattern used by the command
// context, prompt submit, and agent picker.

import { useStore } from "../store/index.js";

import type { OpenletClient } from "../api/client.js";
import type { SessionDto } from "../api/types.js";

// Create a session against the given (or default) agent, register it in the
// store, and make it active. Returns the created session, or null when no agent
// is available (nothing to bind the session to). Callers that only need the
// side effect can ignore the return value.
export async function createAndActivateSession(
  client: OpenletClient,
  agentId?: string,
): Promise<SessionDto | null> {
  const resolvedAgentId = agentId ?? useStore.getState().agents[0]?.id;
  if (!resolvedAgentId) return null;
  const session = await client.createSession({ agent_id: resolvedAgentId });
  const fresh = useStore.getState();
  fresh.setSessions([...Object.values(fresh.sessions), session]);
  fresh.setActiveSession(session.id);
  return session;
}
