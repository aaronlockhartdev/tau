<script module>
// Shared vi.mock factory for store.svelte in component tests.
//
// The store must be a genuine $state proxy: the components' $derived/$effect
// graphs track it reactively, so a plain object would render once and never
// update. The command surface is vi.fn()s — tests assert the wire calls the
// UI made, and stub return values per test.
//
// Plain JS on purpose: a `.svelte` module script is not type-checked, and a
// `.svelte.ts` rename would force `allowImportingTsExtensions` into the
// app's tsconfig (out of scope for the test work).
import { vi } from 'vitest';
import { makeStub } from '../sessions';

export function paneDefault() {
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
    workspaces: [],
    current: null,
    tailJump: 0,
    sessions: {},
    loading: false,
    error: null,
    renderRange: '',
    renderMs: 0,
    skills: {},
    files: {},
    fileErrors: {},
    pane: {},
    tabSelected: [],
    tabSelAnchor: null
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
export const newSession = vi.fn(async () => null);
export const openSessionById = vi.fn(async () => {});
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

export function pane(ws) {
  if (!ws) return null;
  return store.pane[ws] ?? null;
}
export function ensurePane(ws) {
  if (!store.pane[ws]) {
    store.pane[ws] = paneDefault();
  }
  return store.pane[ws];
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
  openSessionById,
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

export function resetMockStore(partial = {}) {
  const d = freshDefaults();
  Object.assign(store, d, partial);
  for (const fn of commandFns) fn.mockReset();
  windowTitle.mockImplementation(() => 'tau');
}

// A full SessionState from a meta plus optional overrides — built with the
// real makeStub so the shape stays honest.
// opts: { parent, state, waitingOn, mru, over }
export function seedState(meta, opts = {}) {
  const { parent = null, state = 'idle', waitingOn = null, mru = 1, over = {} } = opts;
  return { ...makeStub(meta, parent, state, waitingOn, mru), ...over };
}
</script>
