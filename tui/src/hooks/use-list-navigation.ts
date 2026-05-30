// Shared keyboard navigation for vertical list pickers (agents, sessions).
// Clamps the cursor to [0, length) and wires Up/Down/Enter/Esc. Both
// pickers rendered the exact same useInput block before this hook.

import { useInput } from "ink";
import { useState } from "react";

export interface ListNavigation {
  index: number;
}

export function useListNavigation<T>(
  items: T[],
  onSelect: (item: T) => void,
  onCancel: () => void,
): ListNavigation {
  const [index, setIndex] = useState(0);

  useInput((_input, key) => {
    if (key.escape) {
      onCancel();
      return;
    }
    if (key.upArrow) setIndex((i) => Math.max(0, i - 1));
    if (key.downArrow) setIndex((i) => Math.min(items.length - 1, i + 1));
    if (key.return) {
      const choice = items[index];
      if (choice) onSelect(choice);
    }
  });

  return { index };
}
