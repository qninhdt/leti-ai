// Pure grouping helper for the background task panel (Phase 6). The panel lists
// every subagent task for a session tree, grouped by lifecycle state so the
// user can scan running vs. promoted vs. settled at a glance. Split from the
// renderer so the grouping logic is unit-testable without the opentui runtime.

import type { SubagentView } from "../store/types.js";

export interface PanelGroups {
  /// Still executing (status running, not yet promoted).
  running: SubagentView[];
  /// Promoted (auto-notify on settle) and still running.
  promoted: SubagentView[];
  /// Terminal (finished / cancelled / failed).
  settled: SubagentView[];
}

/// Partition the subagents slice into running / promoted / settled buckets.
/// A promoted-but-still-running task goes in `promoted` (not `running`) so the
/// user sees the auto-notify affordance; once terminal it moves to `settled`
/// regardless of promotion. Each bucket is name-stable (sorted by task_id) so
/// the panel doesn't reshuffle rows between renders.
export function groupSubagents(subagents: Record<string, SubagentView>): PanelGroups {
  const running: SubagentView[] = [];
  const promoted: SubagentView[] = [];
  const settled: SubagentView[] = [];
  for (const row of Object.values(subagents)) {
    if (row.status !== "running") settled.push(row);
    else if (row.promoted) promoted.push(row);
    else running.push(row);
  }
  const byId = (a: SubagentView, b: SubagentView) => a.task_id.localeCompare(b.task_id);
  running.sort(byId);
  promoted.sort(byId);
  settled.sort(byId);
  return { running, promoted, settled };
}
