import React from "react";
import { Box, Text } from "ink";
import { marked, type Token } from "marked";

import { theme } from "../theme/index.js";
import { normalizeNestedFences } from "../utils/markdown-walker.js";

export interface MarkdownRendererProps {
  source: string;
  streaming?: boolean;
}

interface Block {
  key: string;
  node: React.ReactElement;
}

// Markdown -> Ink elements. We walk marked's lexer tokens; each top-level
// block becomes one React node. Streaming consumers should pass already-
// finalized text only — pending tail belongs in a plain <Text>.
export function MarkdownRenderer(props: MarkdownRendererProps): React.ReactElement {
  const blocks = React.useMemo(() => renderBlocks(props.source), [props.source]);
  return (
    <Box flexDirection="column">
      {blocks.map((b) => (
        <React.Fragment key={b.key}>{b.node}</React.Fragment>
      ))}
    </Box>
  );
}

function renderBlocks(src: string): Block[] {
  if (!src) return [];
  const tokens = marked.lexer(normalizeNestedFences(src));
  return tokens.map((tok, i) => ({
    key: `${tok.type}-${i}`,
    node: renderToken(tok, i),
  }));
}

function renderToken(tok: Token, idx: number): React.ReactElement {
  const k = `tok-${idx}`;
  switch (tok.type) {
    case "heading": {
      const headingTok = tok as Token & { depth: number; text: string };
      const level = Math.max(1, Math.min(6, headingTok.depth));
      const color = theme.text.heading[level - 1] ?? theme.text.primary;
      return (
        <Box key={k}>
          <Text color={color} bold>{headingTok.text}</Text>
        </Box>
      );
    }
    case "paragraph": {
      const t = tok as Token & { text: string };
      return <Text key={k} color={theme.text.primary}>{t.text}</Text>;
    }
    case "code": {
      const t = tok as Token & { text: string; lang?: string };
      return (
        <Box
          key={k}
          flexDirection="column"
          borderStyle="round"
          borderColor={theme.code.border}
        >
          {(t.lang ? [t.lang] : []).map((lang, j) => (
            <Text key={`lang-${j}`} color={theme.code.border}>{lang}</Text>
          ))}
          <Text color={theme.text.primary}>{t.text}</Text>
        </Box>
      );
    }
    case "blockquote": {
      const t = tok as Token & { text?: string };
      return (
        <Box key={k}>
          <Text color={theme.quote.bar}>│ </Text>
          <Text color={theme.quote.text}>{(t.text ?? "").trim()}</Text>
        </Box>
      );
    }
    case "list": {
      const t = tok as Token & { ordered: boolean; items: Array<{ text: string }> };
      return (
        <Box key={k} flexDirection="column">
          {t.items.map((item: { text: string }, j: number) => (
            <Text key={`li-${j}`} color={theme.text.primary}>
              {t.ordered ? `${j + 1}. ` : "• "}
              {item.text}
            </Text>
          ))}
        </Box>
      );
    }
    case "space":
      return <Text key={k}> </Text>;
    case "hr":
      return <Text key={k} color={theme.border.muted}>───</Text>;
    default:
      return <Text key={k} color={theme.text.primary}>{(tok as { raw?: string }).raw ?? ""}</Text>;
  }
}
