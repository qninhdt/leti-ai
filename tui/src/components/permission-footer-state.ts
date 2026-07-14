import type { PermissionReplyKind } from "../api/types.js";

export type PermissionFooterStage = "permission" | "always" | "reject";

export interface PermissionFooterState {
  stage: PermissionFooterStage;
  selected: number;
  feedback: string;
}

export interface PermissionFooterTransition {
  state: PermissionFooterState;
  reply?: { decision: PermissionReplyKind; reason?: string };
}

export function initialPermissionFooterState(): PermissionFooterState {
  return { stage: "permission", selected: 0, feedback: "" };
}

export function shiftPermissionChoice(
  state: PermissionFooterState,
  delta: -1 | 1,
): PermissionFooterState {
  const count = state.stage === "always" ? 2 : 3;
  return { ...state, selected: (state.selected + delta + count) % count };
}

export function activatePermissionChoice(
  state: PermissionFooterState,
): PermissionFooterTransition {
  if (state.stage === "reject") {
    const reason = state.feedback.trim();
    return {
      state,
      reply: { decision: "deny", reason: reason || undefined },
    };
  }
  if (state.stage === "always") {
    return state.selected === 0
      ? { state, reply: { decision: "always_allow" } }
      : { state: { ...state, stage: "permission", selected: 1 } };
  }
  if (state.selected === 0) return { state, reply: { decision: "allow" } };
  if (state.selected === 1) {
    return { state: { ...state, stage: "always", selected: 0 } };
  }
  return { state: { stage: "reject", selected: 2, feedback: "" } };
}

export function escapePermissionStage(state: PermissionFooterState): PermissionFooterState {
  if (state.stage === "always") return { ...state, stage: "permission", selected: 1 };
  if (state.stage === "reject") return { ...state, stage: "permission", selected: 2 };
  return { ...state, stage: "reject", selected: 2, feedback: "" };
}
