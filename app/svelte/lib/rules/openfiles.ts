// The file viewer's open-file set (the open-files model): a per-workspace list
// of closable tabs, independent of git status. The store keeps the state; this
// module owns the pure list rules (add, close, and which neighbour tab takes
// the active slot). Data in, data out — it never imports the store.

export interface OpenFile {
  path: string;
  /** .gitignored: no committed base (rendered plain, not all-added). */
  ignored: boolean;
  /** Size in bytes at open time: the binary-file message's count. */
  size: number;
}

export type OpenFiles = Record<string, OpenFile[]>;

// Add a file to the open set; a no-op (added: false) when it is already open.
export function openIn(list: OpenFile[], file: OpenFile): { list: OpenFile[]; added: boolean } {
  if (list.some((f) => f.path === file.path)) return { list, added: false };
  return { list: [...list, file], added: true };
}

// Close a tab. The survivor list plus the index of the tab that takes the
// active slot: the closed tab's own index, clamped to the last survivor (a
// left neighbour, else the right, is kept in view). null when the last file
// closes — the caller returns the center to the transcript.
export function closeIn(list: OpenFile[], path: string): { list: OpenFile[]; activate: number | null } {
  const idx = list.findIndex((f) => f.path === path);
  if (idx < 0) return { list, activate: null };
  const next = list.filter((_, i) => i !== idx);
  return { list: next, activate: next.length === 0 ? null : Math.min(idx, next.length - 1) };
}
