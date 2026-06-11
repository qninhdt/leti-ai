// Resolves @-mentions into embeddable prompt content + badge descriptors at
// submit time. For each mention path it calls the server's getFileContent and:
//   - text → a fenced block (```<lang>) headed by the path, appended to the
//     prompt so the model receives the file content;
//   - unsupported (image/pdf/binary) → no content, just a badge flagged
//     unsupported;
//   - missing/denied/errored → recorded as an error badge so the caller can
//     toast it; the mention stays as plain `@path` text (never silently
//     dropped).
// The visible user message shows the badges (chips), NOT the raw dump — only
// the outgoing prompt carries the embedded content.

import { allMentions } from "../utils/mention-parser.js";

import type { OpenletClient } from "../api/client.js";
import type { FileKindDto } from "../api/types.js";

export interface FileBadge {
  path: string;
  kind: FileKindDto;
  /// True when content could not be embedded (binary, missing, error).
  unsupported: boolean;
  truncated: boolean;
  /// Error message when resolution failed (drives a toast).
  error?: string;
}

export interface EmbedResult {
  /// Text to append to the user's prompt (fenced file blocks), or "".
  promptSection: string;
  /// Badge descriptors for the optimistic user message.
  badges: FileBadge[];
}

const LANG_BY_EXT: Record<string, string> = {
  ts: "ts",
  tsx: "tsx",
  js: "js",
  jsx: "jsx",
  rs: "rust",
  py: "python",
  go: "go",
  json: "json",
  toml: "toml",
  md: "md",
  sh: "bash",
};

function langForPath(path: string): string {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return LANG_BY_EXT[ext] ?? "";
}

/// Resolve every mention in `buffer` and build the prompt section + badges.
/// Mentions resolve in parallel; order in the prompt follows buffer order.
export async function embedMentions(buffer: string, client: OpenletClient): Promise<EmbedResult> {
  const mentions = allMentions(buffer);
  if (mentions.length === 0) return { promptSection: "", badges: [] };

  // Dedupe by path so the same file mentioned twice is fetched once.
  const paths = [...new Set(mentions.map((m) => m.path))];
  const resolved = await Promise.all(
    paths.map(async (path): Promise<FileBadge & { content?: string }> => {
      try {
        const res = await client.getFileContent(path);
        return {
          path,
          kind: res.type,
          unsupported: res.unsupported === true || res.content === undefined,
          truncated: res.truncated === true,
          content: res.content,
        };
      } catch (err) {
        return {
          path,
          kind: "text",
          unsupported: true,
          truncated: false,
          error: err instanceof Error ? err.message : String(err),
        };
      }
    }),
  );

  const sections: string[] = [];
  const badges: FileBadge[] = [];
  for (const r of resolved) {
    const { content, ...badge } = r;
    badges.push(badge);
    if (content !== undefined && !badge.unsupported) {
      sections.push(`@${r.path}:\n\`\`\`${langForPath(r.path)}\n${content}\n\`\`\``);
    }
  }

  return {
    promptSection: sections.length > 0 ? `\n\n${sections.join("\n\n")}` : "",
    badges,
  };
}
