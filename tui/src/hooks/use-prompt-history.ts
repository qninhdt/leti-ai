// Prompt history persisted as JSONL append-only at the OS state dir.
// Cap 200 entries.

import { existsSync, mkdirSync, readFileSync, appendFileSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

const CAP = 200;

export class PromptHistory {
  private entries: string[] = [];

  constructor(private file: string) {
    mkdirSync(dirname(file), { recursive: true });
    if (existsSync(file)) {
      const txt = readFileSync(file, "utf8");
      this.entries = txt
        .split("\n")
        .filter((l) => l.trim())
        .map((l) => {
          try {
            return JSON.parse(l) as string;
          } catch {
            return "";
          }
        })
        .filter(Boolean);
    }
  }

  push(text: string): void {
    if (!text.trim()) return;
    if (this.entries[this.entries.length - 1] === text) return;
    this.entries.push(text);
    appendFileSync(this.file, JSON.stringify(text) + "\n");
    if (this.entries.length > CAP) this.compact();
  }

  list(): string[] {
    return this.entries.slice();
  }

  private compact(): void {
    this.entries = this.entries.slice(-CAP);
    writeFileSync(this.file, this.entries.map((e) => JSON.stringify(e)).join("\n") + "\n");
  }
}
