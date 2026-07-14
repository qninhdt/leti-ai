import type { PermissionRequestDto } from "../api/types.js";

export function findSessionPermission(
  pending: Record<string, PermissionRequestDto>,
  sessionId: string | null,
): PermissionRequestDto | undefined {
  if (!sessionId) return undefined;
  return Object.values(pending).find((request) => request.session_id === sessionId);
}
