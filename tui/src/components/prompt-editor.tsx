import React from "react";
import { Box, Text, useInput } from "ink";

import { theme } from "../theme/index.js";

export interface PromptEditorProps {
  value: string;
  onChange: (value: string) => void;
  onSubmit: (value: string) => void;
  onHistoryUp: () => void;
  onHistoryDown: () => void;
  onTabComplete: () => void;
  placeholder?: string;
}

// Multi-line editor on Ink's useInput. Shift+Enter inserts newline
// (Option+Enter on macOS terminals that don't distinguish modifier on
// Enter — documented in tui/README.md). Up at line 0 / Down at last
// line walks history. Tab triggers completion via parent.
export function PromptEditor(props: PromptEditorProps): React.ReactElement {
  const { value, onChange, onSubmit, onHistoryUp, onHistoryDown, onTabComplete } = props;

  useInput((input, key) => {
    if (key.return) {
      if (key.shift || key.meta) {
        onChange(value + "\n");
        return;
      }
      const submission = value;
      onChange("");
      onSubmit(submission);
      return;
    }
    if (key.tab) {
      onTabComplete();
      return;
    }
    if (key.upArrow) {
      onHistoryUp();
      return;
    }
    if (key.downArrow) {
      onHistoryDown();
      return;
    }
    if (key.backspace || key.delete) {
      onChange(value.slice(0, -1));
      return;
    }
    if (key.ctrl || key.escape) return;
    if (input) onChange(value + input);
  });

  const display = value || (props.placeholder ?? "Type a message — Enter to send, Shift+Enter for newline");
  return (
    <Box borderStyle="single" borderColor={theme.border.active} paddingX={1}>
      <Text color={value ? theme.text.primary : theme.text.muted}>{display}</Text>
      <Text color={theme.border.active}>▌</Text>
    </Box>
  );
}
