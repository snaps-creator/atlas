import type { Settings, Snapshot } from "./types";

type Change = { path: string[]; value: unknown };
const object = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
function changes(before: unknown, after: unknown, path: string[] = []): Change[] {
  if (JSON.stringify(before) === JSON.stringify(after)) return [];
  if (object(before) && object(after)) return Object.keys(after).flatMap(key => changes(before[key], after[key], [...path, key]));
  return [{ path, value: after }];
}
function apply(settings: Settings, patch: Change[]): Settings {
  const next = structuredClone(settings);
  for (const { path, value } of patch) {
    let target = next as unknown as Record<string, unknown>;
    for (const key of path.slice(0, -1)) target = target[key] as Record<string, unknown>;
    target[path[path.length - 1]] = structuredClone(value);
  }
  return next;
}

// Only the user's changed fields are rebased. Repository references remain
// backend-owned; a failed save rolls back its own optimistic changes.
export class SettingsQueue {
  private pending: { patch: Change[]; resolve: () => void; reject: (error: unknown) => void }[] = [];
  private running = false;
  constructor(private read: () => Promise<Snapshot>, private write: (settings: Settings, revision: number) => Promise<Snapshot>,
    private commit: (snapshot: Snapshot) => void, private changed: () => void) {}
  overlay(settings: Settings): Settings { return this.pending.reduce((value, item) => apply(value, item.patch), settings); }
  push(before: Settings, after: Settings): Promise<void> {
    const patch = changes(before, after);
    if (!patch.length) return Promise.resolve();
    const result = new Promise<void>((resolve, reject) => this.pending.push({ patch, resolve, reject }));
    this.changed(); void this.drain(); return result;
  }
  private async drain() {
    if (this.running) return;
    this.running = true;
    try {
      while (this.pending.length) {
        const item = this.pending[0];
        try {
          // A selection, subscription refresh or disconnect may have committed
          // while this item waited. Fetch the actual revision before writing.
          let snapshot = await this.read();
          try { snapshot = await this.write(apply(snapshot.settings, item.patch), snapshot.revision ?? 0); }
          catch (error) {
            if (!String(error).includes("Настройки уже изменились.")) throw error;
            snapshot = await this.read();
            snapshot = await this.write(apply(snapshot.settings, item.patch), snapshot.revision ?? 0);
          }
          this.commit(snapshot); item.resolve();
        } catch (error) { item.reject(error); }
        finally { this.pending.shift(); this.changed(); }
      }
    } finally { this.running = false; }
  }
}
