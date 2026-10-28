// The files pane (ticket #32): directory listings, their change-driven
// refetch wave, and the git status that tints the tree. The state lives in the
// bag (state.svelte); this module owns the round-trips that fill it.
import { command } from '../protocol';
import { errText } from '../errors';
import { putListing, scheduleRefetch as queueRefetch, toggleDir, waveDirs } from '../files';
import { store } from './state.svelte';

// One directory's listing into the store. The change guard keeps an
// unchanged refetch a no-op (no reactive churn, no re-render).
export async function fetchDir(ws: string, path: string): Promise<void> {
  let out;
  try {
    out = await command({ type: 'file_list', workspace: ws, path });
  } catch (e) {
    // The workspace may be closing mid-flight: a fetch for a gone
    // workspace is dropped, not an error.
    if (!store.workspaces.some((w) => w.id === ws)) return;
    const failed = store.fileErrors[ws] ?? {};
    store.fileErrors = { ...store.fileErrors, [ws]: { ...failed, [path]: errText(e) } };
    return;
  }
  if (out.kind !== 'files') return;
  store.files = putListing(store.files, ws, path, out.files);
  const failed = store.fileErrors[ws];
  if (failed && path in failed) {
    const next = { ...failed };
    delete next[path];
    store.fileErrors = { ...store.fileErrors, [ws]: next };
  }
}

// Tell the core to stop watching a dir the pane just collapsed (#115): the
// tree watcher tracks exactly the expanded dirs, so a collapse drops the
// watch. Fire-and-forget — a failure (e.g. the workspace closing) is a
// no-op, not an error.
async function unlistDir(ws: string, path: string): Promise<void> {
  try {
    await command({ type: 'file_unlist', workspace: ws, path });
  } catch {
    // a collapse for a gone workspace is a no-op
  }
}

// The tree's change tints and the changes view's switcher refresh on open
// and on every file-tree change. A non-git workspace records null (the
// tints simply stay off), never an error.
export async function refreshGitStatus(ws: string): Promise<void> {
  try {
    const out = await command({ type: 'git_status', workspace: ws });
    store.gitStatus = {
      ...store.gitStatus,
      [ws]: out.kind === 'git_status' ? out.status : null
    };
  } catch {
    store.gitStatus = { ...store.gitStatus, [ws]: null };
  }
}

// Expansion: listed dirs collapse back (drop the fetch); unlisted dirs
// fetch on first expand. A dir is listed only while expanded, so the
// tree's memory tracks what is on screen.
export function toggleFileDir(ws: string, path: string): void {
  const t = toggleDir(store.files, ws, path);
  store.files = t.cache;
  if (t.fetch) void fetchDir(ws, t.fetch);
  else void unlistDir(ws, path);
}

// 'failed to list — click to retry': a failed dir is already listed
// (empty), so toggleFileDir would collapse it — re-fetch instead.
export function retryDirFetch(ws: string, path: string): void {
  void fetchDir(ws, path);
}

const refetchPending = new Map<string, Set<string>>();
let refetchTimer: ReturnType<typeof setTimeout> | null = null;

export function scheduleRefetch(ws: string, dirs: string[]): void {
  queueRefetch(store.files, refetchPending, ws, dirs);
  if (refetchTimer) clearTimeout(refetchTimer);
  refetchTimer = setTimeout(flushRefetch, 300);
}

function flushRefetch(): void {
  refetchTimer = null;
  for (const { ws, path } of waveDirs(store.files, refetchPending)) void fetchDir(ws, path);
}
