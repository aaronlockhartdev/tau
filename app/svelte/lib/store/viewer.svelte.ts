// The file viewer (open-files model): the per-workspace set of open files,
// the active tab, and the content + git base that feeds the diff. The state
// lives in the bag (state.svelte); this module owns the tab lifecycle.
import { command } from '../wire/protocol';
import { errText } from '../wire/errors';
import { closeIn, openIn } from '../rules/openfiles';
import { ensurePane, store } from './state.svelte';

// Open a file in the viewer: adds it to the workspace's open set (a no-op if
// already open), makes it the active tab, and switches the center to the file
// viewer. pane.center is what keeps the files alive across a switch to the
// transcript (the first-class toggle).
export function openFile(ws: string, path: string, ignored: boolean, size: number): void {
  const { list } = openIn(store.openFiles[ws] ?? [], { path, ignored, size });
  store.openFiles = { ...store.openFiles, [ws]: list };
  store.activeFile = { workspace: ws, path };
  ensurePane(ws).center = 'files';
  void fetchActiveFile(ws, path);
}

export function resetActiveFile(): void {
  store.activeFile = null;
  store.activeFileNew = null;
  store.activeFileBase = null;
  store.activeFileError = null;
}

// Close a tab: drop it from the open set. The neighbour tab takes the active
// slot; closing the last file returns the center to the transcript.
export function closeFile(ws: string, path: string): void {
  const { list, activate } = closeIn(store.openFiles[ws] ?? [], path);
  store.openFiles = { ...store.openFiles, [ws]: list };
  const a = store.activeFile;
  if (!a || a.workspace !== ws || a.path !== path) return;
  if (activate === null) {
    resetActiveFile();
    ensurePane(ws).center = 'chat';
    return;
  }
  const next = list[activate]!;
  store.activeFile = { workspace: ws, path: next.path };
  void fetchActiveFile(ws, next.path);
}

// Activate an already-open tab (the tab strip's click).
export function selectFile(ws: string, path: string): void {
  if (!(store.openFiles[ws] ?? []).some((f) => f.path === path)) return;
  store.activeFile = { workspace: ws, path };
  ensurePane(ws).center = 'files';
  void fetchActiveFile(ws, path);
}

// The center-column toggle: transcript ⇄ file viewer. The open set is
// untouched, so a switch to the transcript and back restores the active file.
export function setCenterMode(ws: string, mode: 'chat' | 'files'): void {
  const p = ensurePane(ws);
  if (p.center === mode) return;
  p.center = mode;
  if (mode === 'files' && store.activeFile?.workspace !== ws) {
    const first = (store.openFiles[ws] ?? [])[0];
    if (first) {
      store.activeFile = { workspace: ws, path: first.path };
      void fetchActiveFile(ws, first.path);
    }
  }
}

// A session switch can move us to a workspace the single global activeFile
// no longer belongs to. Re-point it to that workspace's active tab — or clear
// it and drop the center to the transcript if nothing is open there — so the
// viewer never shows a foreign file.
export function reconcileActiveFile(ws: string): void {
  if (ws === '' || store.activeFile?.workspace === ws) return;
  const first = (store.openFiles[ws] ?? [])[0];
  if (first) {
    store.activeFile = { workspace: ws, path: first.path };
    void fetchActiveFile(ws, first.path);
    return;
  }
  resetActiveFile();
  ensurePane(ws).center = 'chat';
}

// The active tab's content + git base. A file_read rejection is a real reason
// (binary / unreadable), not an empty file: it is surfaced, not swallowed. A
// stale fetch must not clobber the tab the user moved on from (field compare,
// since the $state proxy breaks ===).
export async function fetchActiveFile(ws: string, path: string): Promise<void> {
  const entry = (store.openFiles[ws] ?? []).find((f) => f.path === path);
  const ignored = entry?.ignored ?? false;
  const size = entry?.size ?? 0;
  let file: string;
  try {
    const out = await command({ type: 'file_read', workspace: ws, path, offset: null, limit: null });
    file = out.kind === 'file' ? out.file.text : '';
  } catch (e) {
    if (store.activeFile?.workspace !== ws || store.activeFile?.path !== path) return;
    store.activeFileNew = null;
    store.activeFileBase = null;
    store.activeFileError = isBinaryError(errText(e)) ? `binary file · ${size} bytes` : `cannot read: ${errText(e)}`;
    return;
  }
  // A gitignored file has no committed base; diffing it against empty would
  // paint it all-added. Render it plain (base = current content) instead.
  const base = ignored
    ? file
    : await command({ type: 'git_show', workspace: ws, path }).then((o) => (o.kind === 'git_show' ? o.text : '')).catch(() => '');
  if (store.activeFile?.workspace !== ws || store.activeFile?.path !== path) return;
  store.activeFileNew = file;
  store.activeFileBase = base;
  store.activeFileError = null;
}

// The core rejects non-UTF-8 files (its line reader errors on invalid bytes);
// that rejection is a binary file, not a read failure.
function isBinaryError(msg: string): boolean {
  return /utf-?8/i.test(msg);
}
