// File-attachment badge chip, shown on a user message for each @-mention.
// Colored by file type (image→accent, pdf→primary, text→secondary), with the
// path as a chip on the element background — mirrors OpenCode's file badges.
// Unsupported attachments are dimmed and tagged so the user sees the file was
// referenced but its content was not embedded.

import { theme } from "../theme/index.js";

import type { FileBadge } from "../services/attachment-embedder.js";

export interface FileBadgeChipProps {
  badge: FileBadge;
}

function typeColor(badge: FileBadge): string {
  const oc = theme.oc;
  if (badge.unsupported) return oc.textMuted;
  if (badge.kind === "image") return oc.accent;
  if (badge.kind === "pdf") return oc.primary;
  return oc.secondary;
}

function typeTag(badge: FileBadge): string {
  if (badge.kind === "image") return "img";
  if (badge.kind === "pdf") return "pdf";
  return "txt";
}

export function FileBadgeChip(props: FileBadgeChipProps) {
  const oc = theme.oc;
  return (
    <box flexDirection="row">
      <text bg={typeColor(props.badge)} fg={oc.background}>
        {" "}
        {typeTag(props.badge)}{" "}
      </text>
      <text bg={oc.backgroundElement} fg={oc.textMuted}>
        {" "}
        {props.badge.path}
        {props.badge.truncated ? " (truncated)" : ""}
        {props.badge.unsupported ? " (unsupported)" : ""}{" "}
      </text>
    </box>
  );
}
