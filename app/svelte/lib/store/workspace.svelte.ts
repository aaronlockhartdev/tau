// The workspace lifecycle (ticket #25): boot, open, close, redirect, and the
// tab strip's multiselect. The state lives in the bag (state.svelte); this
// module owns the round-trips that move it.
import {
  command,
  isTauri,
  type SessionMeta,
  type Workspace
} from '../protocol';
import { errText } from '../errors';
import { open as pickDirectory } from '@tauri-apps/plugin-dialog';
import { listen } from '@tauri-apps/api/event';
import { applySessionList } from '../sessions';
import { switchSession } from './commands';
import { ensureWorkspace, removeWorkspace } from '../files';
import { applyTabSelect } from '../panes';
import { clearError, store } from './state.svelte';
import { fetchDir, refreshGitStatus } from './files-pane.svelte';
import { resetActiveFile } from './viewer.svelte';

/** Connect: list workspaces, then open the first (or the ?workspace= one). */
export async function init(): Promise<void> {
  if (store.loading) return;
  store.loading = true;
  store.error = null;
  try {
    const params = new URLSearchParams(window.location.search);
    // Outside the Tauri shell there is no core to talk to; invoke would
    // hang forever, so fail fast.
    if (!isTauri()) {
      store.error = 'no Tauri window — run the app';
      return;
    }
    // File → Open Folder… (native menu, ticket #29 B1): the picker runs
    // the dialog plugin's proven path (incl. scope handling); the store
    // just opens the result as a workspace.
    void listen('open_folder_requested', () => {
      void openFolderFlow();
    });
    async function openFolderFlow() {
      try {
        const picked = await pickDirectory({ directory: true, multiple: false });
        const dir = Array.isArray(picked) ? picked[0] : picked;
        if (typeof dir !== 'string' || dir === '') return;
        const name = dir.split(/[\\/]/).filter(Boolean).pop() ?? dir;
        void openWorkspace({ id: '', name, cwd: dir });
      } catch (e) {
        store.error = errText(e);
      }
    }
    const out = await command({ type: 'workspace_list' });
    if (out.kind !== 'workspaces') throw new Error('unexpected workspace_list output');
    store.workspaces = out.workspaces;
    const wanted = params.get('workspace');
    const first =
      (wanted ? out.workspaces.find((w) => w.name === wanted || w.id === wanted) : undefined) ??
      out.workspaces[0] ??
      null;
    if (first === null) return;
    await openWorkspace(first);
  } catch (e) {
    store.error = errText(e);
  } finally {
    store.loading = false;
  }
}

// In-flight marker: the open's own workspace_opened event re-enters the
// store via syncWorkspaces; without the marker it re-opens mid-flight,
// and on an empty workspace the two session_new calls create a phantom
// duplicate session. A same-tab double-click shares the in-flight open;
// a different tab awaits it and then proceeds (a dropped tab click was
// a lost navigation).
let inFlight: { cwd: string; done: Promise<void> } | null = null;

export async function openWorkspace(ws: Workspace): Promise<void> {
  if (inFlight) {
    if (inFlight.cwd === ws.cwd) return inFlight.done;
    await inFlight.done;
  }
  const done = (async () => {
    try {
      await openWorkspaceInner(ws);
    } catch (e) {
      store.error = errText(e);
    }
  })();
  inFlight = { cwd: ws.cwd, done };
  try {
    await done;
  } finally {
    if (inFlight?.done === done) inFlight = null;
  }
}

async function openWorkspaceInner(ws: Workspace): Promise<void> {
  clearError();
  // The core keys workspaces by cwd (deterministic id), so re-opening a
  // folder returns the same workspace; the store keeps one tab per cwd.
  const opened = await command({ type: 'workspace_open', cwd: ws.cwd });
  const real = opened.kind === 'workspace' ? opened.workspace : ws;
  // The skill registry refreshes per open/switch (ticket #28): discovery
  // is cheap and the cache keys on the workspace, so a stale list never
  // outlives a tab.
  // skill_list and session_list only depend on the workspace id, so
  // both fire at once (one round-trip, not two) before the store updates.
  const [skillsOut, list] = await Promise.all([
    command({ type: 'skill_list', workspace: real.id }),
    command({ type: 'session_list', workspace: real.id })
  ]);
  if (skillsOut.kind === 'skills') store.skills[real.id] = skillsOut.skills;
  const i = store.workspaces.findIndex((w) => w.cwd === real.cwd);
  if (i >= 0) {
    store.workspaces[i] = real;
  } else {
    store.workspaces.push(real);
  }
  // The listed sessions materialize as stubs so the pane shows them all;
  // entries hydrate lazily when one is opened.
  if (list.kind === 'sessions') store.sessions = applySessionList(store.sessions, list.sessions);
  // The pane's first listing (ticket #32): the root, listed on open —
  // every other dir is fetched on expansion.
  store.files = ensureWorkspace(store.files, real.id);
  void fetchDir(real.id, '.');
  void refreshGitStatus(real.id);
  // Archived sessions are listed (for the archive folder) but can't be
  // opened; auto-open the most recent live one, else start a fresh session.
  const unarchived =
    list.kind === 'sessions'
      ? list.sessions.filter((s) => !s.archived)
      : [];
  const latest = unarchived.reduce<SessionMeta | null>(
    (best, s) => (best === null || s.created > best.created ? s : best),
    null
  );
  const sid = latest
    ? latest.id
    : ((await command({
      type: 'session_new',
      workspace: real.id,
      title: null,
      base_prompt: null
    })) as { kind: 'session'; session: SessionMeta }).session.id;
  await switchSession(sid);
}

// A workspace can be opened by any client (this window, a future second
// window, a remote backend): the store re-reads the list and, if nothing
// is open yet, opens the first one — the boot rule, applied live.
export async function syncWorkspaces(): Promise<void> {
  if (inFlight) return;
  try {
    const out = await command({ type: 'workspace_list' });
    if (out.kind !== 'workspaces') return;
    store.workspaces = out.workspaces;
    if (store.current === null) {
      const first = out.workspaces[0];
      if (first) await openWorkspace(first);
    }
  } catch (e) {
    store.error = errText(e);
  }
}

export async function closeWorkspace(ws: Workspace, redirect = true): Promise<void> {
  const list = await command({ type: 'session_list', workspace: ws.id }).catch(() => ({
    kind: 'sessions' as const,
    sessions: []
  }));
  if (list.kind === 'sessions') {
    for (const s of list.sessions) {
      await command({ type: 'session_close', session: s.id }).catch(() => {});
    }
  }
  // Persist the close (B7): the core's `open` flag keeps the tab closed
  // across a restart. A failed persist surfaces the banner; the tab
  // closes locally either way.
  await command({ type: 'workspace_close', workspace: ws.id }).catch((e) => {
    store.error = errText(e);
  });
  delete store.skills[ws.id];
  if (ws.id in store.fileErrors) {
    const next = { ...store.fileErrors };
    delete next[ws.id];
    store.fileErrors = next;
  }
  store.files = removeWorkspace(store.files, ws.id);
  // The closed workspace's open files (and the active file, if it pointed
  // here) are dead with it.
  delete store.openFiles[ws.id];
  if (store.activeFile?.workspace === ws.id) {
    resetActiveFile();
  }
  store.workspaces = store.workspaces.filter((w) => w.id !== ws.id);
  for (const [sid, s] of Object.entries(store.sessions)) {
    if (s.meta.workspace === ws.id) delete store.sessions[sid];
  }
  // The closed workspace's pane view state is dead with it.
  delete store.pane[ws.id];
  // The closed tab drops out of the selection too (no dangling member).
  store.tabSelected = store.tabSelected.filter((x) => x !== ws.id);
  if (redirect) await redirectCurrent();
}

// After one or more workspace closes: if current dangles (its session
// lived in a closed workspace) or is null, re-open the first surviving
// tab so current lands on a real, hydrated session. A map-only redirect
// is not enough — the survivor's sessions may not be in the map yet
// (they hydrate lazily per workspace) — and a dangling current makes the
// next workspace-scoped command fail with 'missing field `workspace`'
// (dogfood B1) while the tabs strand the app on 'No workspace open' (S6).
async function redirectCurrent(): Promise<void> {
  if (store.current !== null && store.sessions[store.current]) return;
  const survivor = store.workspaces[0];
  if (!survivor) {
    store.current = null;
    return;
  }
  await openWorkspace(survivor);
  if (!store.sessions[store.current ?? '']) store.current = null;
}

// The tab strip's multiselect (B2): cmd/ctrl toggles a tab in and out,
// shift spans from the anchor in tab order, a plain click clears the
// selection (the caller then activates the tab).
export function tabSelect(
  ws: string,
  e: { shiftKey: boolean; metaKey: boolean; ctrlKey: boolean }
): void {
  const t = applyTabSelect(
    { selected: store.tabSelected, anchor: store.tabSelAnchor },
    store.workspaces.map((w) => w.id),
    ws,
    e
  );
  store.tabSelected = t.selected;
  store.tabSelAnchor = t.anchor;
}

// A bulk close (B2): N closes sharing ONE final redirect over the
// surviving set — a per-close redirect would re-open tabs that a later
// close in the same batch removes.
export async function closeWorkspaces(wss: Workspace[]): Promise<void> {
  store.tabSelected = [];
  store.tabSelAnchor = null;
  for (const w of wss) await closeWorkspace(w, false);
  await redirectCurrent();
}

export async function setModel(model: string): Promise<void> {
  const sid = store.current;
  if (!sid) return;
  const s = store.sessions[sid];
  if (!s || !model || s.meta.model === model) return;
  try {
    clearError();
    await command({ type: 'session_set_model', session: sid, model });
  } catch (e) {
    store.error = errText(e);
    return;
  }
  s.meta.model = model;
}
