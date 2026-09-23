// The files-pane cache (roadmap C11): listed-dir state per workspace, lazy
// expansion, and the change-coalesced refetch wave. The store keeps the
// file_list command and hands results in; the wave rechecks listing state
// at flush time so a dir collapsed inside the 300 ms window drops out.
import type { FileEntry } from './protocol';

export type FilesCache = Record<string, Record<string, FileEntry[]>>;

export function ensureWorkspace(cache: FilesCache, ws: string): FilesCache {
  if (cache[ws]) return cache;
  return { ...cache, [ws]: {} };
}

export function removeWorkspace(cache: FilesCache, ws: string): FilesCache {
  if (!(ws in cache)) return cache;
  const next = { ...cache };
  delete next[ws];
  return next;
}

// A directory's listing into the cache. The change guard keeps an
// unchanged refetch a no-op (no reactive churn, no re-render).
export function putListing(cache: FilesCache, ws: string, path: string, files: FileEntry[]): FilesCache {
  const cur = cache[ws];
  if (!cur) return cache;
  if (JSON.stringify(cur[path] ?? []) === JSON.stringify(files)) return cache;
  return { ...cache, [ws]: { ...cur, [path]: files } };
}

// Expansion: listed dirs collapse back (drop the fetch); unlisted dirs
// fetch on first expand. A dir is listed only while expanded, so the
// tree's memory tracks what is on screen. The result names the fetch the
// expansion starts (null on a collapse).
export function toggleDir(cache: FilesCache, ws: string, path: string): { cache: FilesCache; fetch: string | null } {
  const cur = cache[ws];
  if (!cur) return { cache, fetch: null };
  if (cur[path]) {
    const next = { ...cur };
    delete next[path];
    return { cache: { ...cache, [ws]: next }, fetch: null };
  }
  return { cache: { ...cache, [ws]: { ...cur, [path]: [] } }, fetch: path };
}

// Invalidation (the design's lost-events case, client side): a burst of
// file_tree_changed events coalesces into one refetch wave; only listed
// (expanded) dirs are refetched — a change in an unlisted dir is fetched
// the moment the user expands it.
export function scheduleRefetch(
  cache: FilesCache,
  pending: Map<string, Set<string>>,
  ws: string,
  dirs: string[]
): void {
  const cur = cache[ws];
  if (!cur) return;
  let set = pending.get(ws);
  if (!set) {
    set = new Set();
    pending.set(ws, set);
  }
  for (const d of dirs) if (cur[d]) set.add(d);
}

// The wave's flush: a dir collapsed inside the 300 ms window is no longer
// listed, so it drops out here, not at schedule time (the listed? guard
// must be fresh).
export function waveDirs(
  cache: FilesCache,
  pending: Map<string, Set<string>>
): Array<{ ws: string; path: string }> {
  const out: Array<{ ws: string; path: string }> = [];
  for (const [ws, dirs] of pending) {
    pending.delete(ws);
    const cur = cache[ws];
    for (const d of dirs) if (cur?.[d]) out.push({ ws, path: d });
  }
  return out;
}
