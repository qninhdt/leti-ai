// Frecency ranking.
// score = frequency * (1 / (1 + daysSinceLastOpen)).
// Sources persist as JSONL append-only at the OS state dir; on load,
// duplicates are de-duped by path (latest wins). Cap 1000.

import { existsSync, mkdirSync, readFileSync, writeFileSync, appendFileSync } from "node:fs";
import { dirname } from "node:path";

const DAY_MS = 86_400_000;
const MAX_ENTRIES = 1000;

export interface FrecencyEntry {
  path: string;
  frequency: number;
  lastOpen: number;
}

function loadAll(file: string): Map<string, FrecencyEntry> {
  if (!existsSync(file)) return new Map();
  const map = new Map<string, FrecencyEntry>();
  const text = readFileSync(file, "utf8");
  for (const line of text.split("\n")) {
    if (!line.trim()) continue;
    try {
      const e = JSON.parse(line) as FrecencyEntry;
      if (e.path && typeof e.frequency === "number" && typeof e.lastOpen === "number") {
        map.set(e.path, e);
      }
    } catch {}
  }
  return map;
}

export function score(entry: FrecencyEntry, now = Date.now()): number {
  const daysSince = (now - entry.lastOpen) / DAY_MS;
  const weight = 1 / (1 + daysSince);
  return entry.frequency * weight;
}

export class FrecencyStore {
  private map = new Map<string, FrecencyEntry>();

  constructor(private file: string) {
    mkdirSync(dirname(file), { recursive: true });
    this.map = loadAll(file);
  }

  get(path: string): number {
    const e = this.map.get(path);
    return e ? score(e) : 0;
  }

  bump(path: string): void {
    const existing = this.map.get(path);
    const entry: FrecencyEntry = {
      path,
      frequency: (existing?.frequency ?? 0) + 1,
      lastOpen: Date.now(),
    };
    this.map.set(path, entry);
    appendFileSync(this.file, JSON.stringify(entry) + "\n");
    if (this.map.size > MAX_ENTRIES) this.compact();
  }

  rank<T extends { path?: string }>(items: T[]): T[] {
    return items
      .map((it) => ({ it, s: it.path ? this.get(it.path) : 0 }))
      .sort((a, b) => b.s - a.s)
      .map((x) => x.it);
  }

  private compact(): void {
    const entries = Array.from(this.map.values()).sort((a, b) => b.lastOpen - a.lastOpen);
    const trimmed = entries.slice(0, MAX_ENTRIES);
    this.map = new Map(trimmed.map((e) => [e.path, e]));
    writeFileSync(this.file, trimmed.map((e) => JSON.stringify(e)).join("\n") + "\n");
  }
}
