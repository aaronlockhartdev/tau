// Shared vi.mock factory for store.svelte in component tests.
//
// The store must be a genuine $state proxy: the components' $derived/$effect
// graphs track it reactively, so a plain object would render once and never
// update. The command surface is vi.fn()s — tests assert the wire calls the
// UI made, and stub return values per test.
//
// A `.svelte.ts` module (not `.svelte`) so svelte-check sees the named
// exports; it needs `allowImportingTsExtensions` in tsconfig.json.
import { vi } from 'vitest';
import type { FilesCache } from '../files';
import type { PaneState } from '../panes';
import type { FileEntry, SkillInfo, SessionMeta, Workspace } from '../protocol';
import { makeStub, type SessionState } from '../sessions';

export function paneDefault(): PaneState {
  return {
    ltab: 'sessions',
    rtab: 'tasks',
    historyOpen: false,
    expandedTasks: [],
    openGroups: null,
    renamingId: null,
    archOpen: false,
    selSub: null,
    selected: [],
    selAnchor: null
  };
}

export function freshDefaults() {
  return {
    focus: false,
    modelMenuOpen: false,
    reasoningOpen: true,
    entryOpen: new Map(),
    workspaces: [] as Workspace[],
    current: null as string | null,
    tailJump: 0,
    sessions: {} as Record<string, SessionState>,
    loading: false,
    error: null as string | null,
    renderRange: '',
    renderMs: 0,
    skills: {} as Record<string, SkillInfo[]>,
    files: {} as FilesCache,
    fileErrors: {} as Record<string, Record<string, string>>,
    pane: {} as Record<string, PaneState>,
    tabSelected: [] as string[],
    tabSelAnchor: null as string | null
  };
}

const store = $state(freshDefaults());

export const mockStore = store;

// The command surface: every export the components import, as a vi.fn.
export const init = vi.fn(async () => {});
export const applyEvents = vi.fn();
export const send = vi.fn(async () => {});
export const stop = vi.fn(async () => {});
export const openWorkspace = vi.fn(async () => {});
export const closeWorkspace = vi.fn(async () => {});
export const closeWorkspaces = vi.fn(async () => {});
export const tabSelect = vi.fn();
export const newSession = vi.fn(() => Promise.resolve(null));
export const switchSession = vi.fn(async () => {});
export const renameSession = vi.fn(async () => {});
export const archiveSession = vi.fn(async () => {});
export const restoreSession = vi.fn(async () => {});
export const deleteSession = vi.fn(async () => {});
export const setModel = vi.fn(async () => {});
export const deleteQueueItem = vi.fn(async () => {});
export const toggleFileDir = vi.fn(async () => {});
export const retryDirFetch = vi.fn(async () => {});
export const fetchWindow = vi.fn(async () => {});
export const toggleAllReasoning = vi.fn();
export const windowTitle = vi.fn(() => 'tau');

export function pane(ws: string | null): PaneState | null {
  if (!ws) return null;
  return store.pane[ws] ?? null;
}
export function ensurePane(ws: string): PaneState {
  if (!store.pane[ws]) {
    store.pane[ws] = paneDefault();
  }
  return store.pane[ws];
}

// Read facade (F2): mirrors the store's current-session / file-cache accessors.
export function currentSession(): SessionState | null {
  const c = store.current;
  return c ? store.sessions[c] ?? null : null;
}

export function fileCache(ws: string): Record<string, FileEntry[]> | undefined {
  return store.files[ws];
}
// Re-seed the same $state object in place (the mock factory runs once per
// test file; a re-assignment would break the components' tracked references)
// and clear the command fns.
const commandFns = [
  init,
  applyEvents,
  send,
  stop,
  openWorkspace,
  closeWorkspace,
  closeWorkspaces,
  tabSelect,
  newSession,
  switchSession,
  renameSession,
  archiveSession,
  restoreSession,
  deleteSession,
  setModel,
  deleteQueueItem,
  toggleFileDir,
  retryDirFetch,
  fetchWindow,
  toggleAllReasoning,
  windowTitle
];

export function resetMockStore(partial: Partial<typeof store> = {}) {
  const d = freshDefaults();
  Object.assign(store, d, partial);
  for (const fn of commandFns) fn.mockReset();
  windowTitle.mockImplementation(() => 'tau');
}

// A full SessionState from a meta plus optional overrides — built with the
// real makeStub so the shape stays honest.
export function seedState(
  meta: SessionMeta,
  opts: {
    parent?: string | null;
    state?: SessionState['state'];
    waitingOn?: string | null;
    mru?: number;
    over?: Partial<SessionState>;
  } = {}
) {
  const { parent = null, state = 'idle', waitingOn = null, mru = 1, over = {} } = opts;
  return { ...makeStub(meta, parent, state, waitingOn, mru), ...over };
}
