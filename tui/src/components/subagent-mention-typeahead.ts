// Pure candidate function for the prompt `@mention` typeahead (Phase 6). The
// typeahead offers two sources, deduped by name:
//   1. STATIC registered agent slugs (the existing agent-picker source) —
//      always available, spawns a fresh subagent;
//   2. LIVE named siblings from the roster slice (Phase 4 `subagent_roster`) —
//      addressable running siblings, annotated so the user can tell them apart.
//
// Split from any renderer so it's unit-testable without the opentui runtime
// (mirrors tool-*-parse.ts). Live entries take precedence on a name clash so a
// running sibling's handle (e.g. `reviewer#2`) is offered over a static slug.

import type { AgentDto } from "../api/types.js";
import type { RosterView } from "../store/types.js";

export interface MentionCandidate {
  /// The text inserted when selected (agent slug or live handle name).
  name: string;
  /// `agent` = static registered def; `live` = running roster sibling.
  kind: "agent" | "live";
  /// Secondary label (agent description, or a live status annotation).
  detail?: string;
}

/// Case-insensitive prefix match on the token typed after `@` (the leading `@`
/// already stripped by the caller). An empty query matches everything.
function matches(name: string, query: string): boolean {
  if (query.length === 0) return true;
  return name.toLowerCase().startsWith(query.toLowerCase());
}

/// Build the deduped, ranked candidate list for a `@<query>` typeahead. Live
/// siblings sort first (they're actionable *now*), then static agents; within
/// each group, name order. A live entry shadows a static agent of the same
/// name (dedup by lowercased name, live wins).
export function mentionCandidates(
  query: string,
  agents: AgentDto[],
  roster: Record<string, RosterView>,
): MentionCandidate[] {
  const live: MentionCandidate[] = Object.values(roster)
    .filter((e) => matches(e.name, query))
    .map((e) => ({ name: e.name, kind: "live" as const, detail: "● running" }));

  const liveNames = new Set(live.map((c) => c.name.toLowerCase()));

  const staticAgents: MentionCandidate[] = agents
    .filter((a) => matches(a.display_name, query) && !liveNames.has(a.display_name.toLowerCase()))
    .map((a) => ({
      name: a.display_name,
      kind: "agent" as const,
      detail: a.description ?? undefined,
    }));

  live.sort((a, b) => a.name.localeCompare(b.name));
  staticAgents.sort((a, b) => a.name.localeCompare(b.name));
  return live.concat(staticAgents);
}
