import type { ReloadScope } from "./types";

export type DataScope = "dashboard" | "nodes" | "settings";

// Merge bursts without discarding another scope. Requests arriving during a
// read run afterwards, so a mutation never waits on a pre-mutation snapshot.
export function createRefreshQueue(load: (scopes: DataScope[]) => Promise<void>) {
  let pending = new Set<DataScope>();
  let waiters: { resolve: () => void; reject: (error: unknown) => void }[] = [];
  let running = false;
  const drain = async () => {
    while (pending.size) {
      const scopes = [...pending];
      const current = waiters;
      pending = new Set();
      waiters = [];
      try { await load(scopes); current.forEach(item => item.resolve()); }
      catch (error) { current.forEach(item => item.reject(error)); }
    }
    running = false;
  };
  return (scope: ReloadScope = "all"): Promise<void> => {
    if (scope === "none") return Promise.resolve();
    (scope === "all" ? ["dashboard", "nodes", "settings"] as const : [scope]).forEach(item => pending.add(item));
    const promise = new Promise<void>((resolve, reject) => waiters.push({ resolve, reject }));
    if (!running) { running = true; queueMicrotask(() => void drain()); }
    return promise;
  };
}

// IPC creates new objects even if their data did not change. Keep identity so
// memoized pages and settings drafts survive idle refreshes.
export function retainSnapshot<T>(previous: T, next: T): T {
  return JSON.stringify(previous) === JSON.stringify(next) ? previous : next;
}
