// Shared session lifecycle actions. Consolidates the duplicated
// "create session + push to store + activate" pattern used by the command
// context, prompt submit, and agent picker.

import { useStore } from "../store/index.js";

import type { OpenletClient } from "../api/client.js";

export async function createAndActivateSession(
  client: OpenletClient,
  agentId?: string,
): Promise<void> {
  const resolvedAgentId = agentId ?? useStore.getState().agents[0]?.id;
  if (!resolvedAgentId) return;
  const session = await client.createSession({ agent_id: resolvedAgentId });
  const fresh = useStore.getState();
  fresh.setSessions([...Object.values(fresh.sessions), session]);
  fresh.setActiveSession(session.id);
}
