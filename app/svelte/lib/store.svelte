<script module lang="ts">
  // App state for the central column (ticket #25). $state lives in a
  // .svelte module file because runes only transform there; components
  // import the exported `store` binding.
  //
  // The GUI is a stateless renderer (ADR-0006, live transport per
  // ADR-0008): every mutation is a command round-trip or an event
  // application, so a rebuild from snapshot + events always converges.
  // The transcript is an id-keyed map — entry_upsert carries the file's
  // own line, paged reads materialize the rest (spec §8).

  import {
    command,
    isTauri,
    type Event,
    type FileEntry,
    type GitStatus,
    type SessionMeta,
    type SkillInfo,
    type Workspace
  } from './protocol';
  import {
    archiveSession,
    deleteSession,
    newSession,
    refetchSessionList,
    renameSession,
    restoreSession,
    runSessionCommand,
    switchSession
  } from './commands';
  export {
    archiveSession,
    deleteSession,
    newSession,
    refetchSessionList,
    renameSession,
    restoreSession,
    runSessionCommand,
    switchSession
  };
  import { presentation } from './presentation.svelte';
  import { resolveBlobs, upsertEntry } from './entries';
  import {
    applySessionList,
    applySubagentEvent,
    laneOf,
    type PendingMsg,
    type SessionState,
    windowTitle as windowTitleOf
  } from './sessions';
  export type { PendingMsg, SessionState } from './sessions';
  import {
    type FilesCache,
    ensureWorkspace,
    putListing,
    removeWorkspace,
    scheduleRefetch as queueRefetch,
    toggleDir,
    waveDirs
  } from './files';
  import { applyTabSelect, defaultPane, paneOf, type PaneState } from './panes';
  import { closeIn, openIn, type OpenFiles } from './openfiles';
  import { errText } from './errors';
  export type { PaneState } from './panes';
  import { listen } from '@tauri-apps/api/event';
  import { getCurrentWindow } from '@tauri-apps/api/window';
  import { attentionOf, type AttentionItem } from './attention';
  import { open as pickDirectory } from '@tauri-apps/plugin-dialog';
  export const store = $state({
    focus: false,
    // The wall clock the relative-age labels read; ticked every 30s so idle
    // ages advance live (each tick rewrites only the label text).
    now: Date.now(),
    // Window fullscreen state: the top bar drops its traffic-light inset when
    // the OS hides the controls (tracked by the module-scope listener below).
    fullscreen: false,
    attentionOpen: false,
    paletteOpen: false,
    fleetOpen: false,
    focusInspector: null as 'left' | 'right' | null,
    // The follow filter (P2): the handle of the sub-agent the transcript is
    // focused on; non-followed entries render dimmed. Cleared by the chip.
    follow: null as string | null,
    modelMenuOpen: false,
    // Seeded from the per-user presentation registry (spec §12, #35):
    // 'visible-by-default' is the user's standing preference, and the 'r'
    // keybind toggles it for the session.
    reasoningOpen: presentation.reasoningVisibleByDefault,
    // Per-entry card expansion, keyed `${session}:${entryId}:${slot}`: the
    // transcript window unmounts off-screen cards, so the state lives here,
    // not in the card's local $state (which would reset to collapsed on
    // remount — scrolling to the bottom collapsed every expanded card).
    // A plain persistence map (not reactive — Svelte 5.57 deep-proxies
    // objects/arrays in $state, not Maps): EntryCard seeds its local
    // reactive state from it on mount and writes toggles back.
    entryOpen: new Map(),
    workspaces: [] as Workspace[],
    current: null as string | null,
    tailJump: 0,
    /* eslint-disable @typescript-eslint/no-unnecessary-type-assertion -- the $state generic accepts the bare literal; the casts pin the inferred type ({} → Record<...>) */
    sessions: {} as Record<string, SessionState>,
    loading: false,
    error: null as string | null,
    // The transcript's last window computation (spec §9 seg3 render stats).
    renderRange: '',
    renderMs: 0,
    // The skill registry, cached per workspace (ticket #28): the
    // composer's /skill: autocomplete data source.
    skills: {} as Record<string, SkillInfo[]>,
    // The files pane (ticket #32), per workspace: the listed dirs and
    // their entries. A dir appears here once fetched (workspace open
    // lists the root, expansion lists its children); FileTreeChanged
    // marks a listed dir stale and a coalesced refetch wave replaces it.
    files: {} as FilesCache,
    // Per-dir file_list failures (the empty-directory deception): a failed
    // listing is recorded per workspace/path and cleared when the same dir
    // lists successfully again.
    fileErrors: {} as Record<string, Record<string, string>>,
    // Git status per workspace (workbench P4): the tree's change tints and
    // the changes view's file switcher. null = not a git repository.
    gitStatus: {} as Record<string, GitStatus | null>,
    // The file viewer (open-files model): a persistent, per-workspace set of
    // open files (closable tabs), independent of git status. The center column
    // toggles between the transcript and the viewer (pane.center); open files
    // survive a switch to the transcript. activeFile* is the shown tab: a null
    // New is "loading", a set Error is a real reason (binary / unreadable), ''
    // is an empty file.
    openFiles: {} as OpenFiles,
    activeFile: null as { workspace: string; path: string } | null,
    activeFileNew: null as string | null,
    activeFileBase: null as string | null,
    activeFileError: null as string | null,
    // Per-tab isolation (spec §9): each open workspace owns its pane view
    // state; the transcript's conversation state stays per-session.
    pane: {} as Record<string, PaneState>,
    /* eslint-enable @typescript-eslint/no-unnecessary-type-assertion */
    // The tab strip's multiselect (B2): the pane's session-select shape
    // (PaneState's selected/selAnchor), but global — a selection can span
    // tabs of several workspaces, so it keys on nothing.
    tabSelected: [] as string[],
    tabSelAnchor: null as string | null
  });

  // Advance the wall clock on a slow tick so relative-age labels (session mru,
  // attention "since") stay current without session activity.
  setInterval(() => {
    store.now = Date.now();
  }, 30_000);

  // The top bar reserves left padding for the macOS traffic lights; they're
  // hidden in fullscreen, so we drop the inset there. Tauri has no
  // fullscreen-changed event, so we track it off the window resize event
  // (tauri-apps/tauri#7162). Entering is clean: isFullscreen() flips true at
  // the animation's start. Exiting is the catch: isFullscreen() stays true
  // until the animation ENDS, by then the lights are back and briefly cover
  // the logo. The resize event, though, fires at the exit animation's START,
  // and a resize while settled in fullscreen can only be that exit (macOS
  // won't let you resize a fullscreen window) — so we restore the inset then,
  // before isFullscreen catches up. The grace window skips the enter
  // animation's own resizes; `exiting` ignores the stale-true mid-exit.
  // A rapid exit inside the grace window is indistinguishable from the enter
  // animation (no pre-transition signal), so it keeps the inset dropped once — cosmetic.
  if (isTauri()) {
    const win = getCurrentWindow();
    let enterAt = 0;
    let exiting = false;
    let exitAt = 0;
    const settle = 1200; // longer than the enter animation, so its resizes fall inside the window
    const sync = () =>
      void win.isFullscreen().then((fs) => {
        if (exiting && fs && Date.now() - exitAt < settle) return; // stale-true only mid-exit
        exiting = false;
        store.fullscreen = fs;
        enterAt = fs ? Date.now() : 0;
      });
    sync();
    void win.onResized(() => {
      if (store.fullscreen && Date.now() - enterAt > settle) {
        exiting = true;
        exitAt = Date.now();
        store.fullscreen = false; // exit animation starting; move the logo clear now
        return;
      }
      sync();
    });
  }

  // The needs-you list (P5): a pure fold over the sessions, so any pane
  // (bell, sheet, tree tint) reads one shared projection. A function —
  // derived state may not be exported from a module.
  export function attention(): AttentionItem[] {
    return attentionOf(store.sessions);
  }


  // Pure read — a $derived may call this; creation goes through ensurePane
  // (a mutation, only from effects/actions).
  export function pane(ws: string | null): PaneState | null {
    return paneOf(store.pane, ws);
  }

  // Create-if-missing (the old pane() behavior) — call from effects/actions.
  export function ensurePane(ws: string): PaneState {
    const prev = store.pane[ws];
    if (prev) return prev;
    const p = defaultPane();
    store.pane[ws] = p;
    return p;
  }

  // Read facade (F2): the current-session and per-workspace pane / file-cache
  // reads the views each re-derived from the raw bag with their own null
  // discipline, centralized so the bag is an implementation detail. Read-only.

  export function currentSession(): SessionState | null {
    const c = store.current;
    return c ? store.sessions[c] ?? null : null;
  }

  export function fileCache(ws: string): Record<string, FileEntry[]> | undefined {
    return store.files[ws];
  }
  export function toggleAllReasoning(): void {
    store.reasoningOpen = !store.reasoningOpen;
  }

  export function setFollow(handle: string | null): void {
    store.follow = handle;
  }

  export function windowTitle(): string {
    return windowTitleOf({
      current: store.current,
      sessions: store.sessions,
      workspaces: store.workspaces
    });
  }

  // Per-call clock (ms epoch) by call_id: TPS = a call's output tokens over
  // its own stream duration, shown in the status bar.
  let callStartMs = new Map<string, number>();
  // Per-session stream_start clock (ms epoch) by session id: the base for
  // the live (estimated) TPS while a call is in flight. The exact output
  // token count lands only at stream_end, so the status bar's gauge runs on
  // a text-length estimate in the meantime.
  let streamStartMs = new Map<string, number>();

  function sessionOf(sid: string): SessionState {
    const s = store.sessions[sid];
    if (!s) throw new Error(`unknown session ${sid}`);
    return s;
  }

  // Banner lifecycle (dogfood N2): every command clears the stale banner
  // before it runs, a failed command sets a fresh one, and non-error
  // system events no longer wipe it (applyEvents).
  function clearError(): void {
    store.error = null;
  }


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

  // Verification seam (the dev rig): feed wire-shaped events through
  // applyEvents and read the current session's live-stream lengths (store level — the
  // virtualized DOM only shows the window, one stream card can sit outside it).
  (window as unknown as { __tau?: unknown }).__tau = {
    applyEvents,
    store: () => store,
    // send/stop/openWorkspace/closeWorkspace/switchSession/fetchWindow
    // are module exports the rig drives directly; hoisted above.
    send,
    stop,
    openWorkspace,
    closeWorkspace,
    switchSession,
    fetchWindow,
    omStatus: (kind: 'observing' | 'reflecting' | 'idle') =>
      applyEvents([
        {
          type: 'om_status',
          workspace: store.current ? store.sessions[store.current]!.meta.workspace : '',
          session: store.current ?? '',
          kind
        }
      ]),
    sessionSetModel: (session: string, model: string) => command({ type: 'session_set_model', session, model })
  };


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

  // One directory's listing into the store. The change guard keeps an
  // unchanged refetch a no-op (no reactive churn, no re-render).
  async function fetchDir(ws: string, path: string): Promise<void> {
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
  async function refreshGitStatus(ws: string): Promise<void> {
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

  function resetActiveFile(): void {
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
  async function fetchActiveFile(ws: string, path: string): Promise<void> {
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

  function scheduleRefetch(ws: string, dirs: string[]): void {
    queueRefetch(store.files, refetchPending, ws, dirs);
    if (refetchTimer) clearTimeout(refetchTimer);
    refetchTimer = setTimeout(flushRefetch, 300);
  }

  function flushRefetch(): void {
    refetchTimer = null;
    for (const { ws, path } of waveDirs(store.files, refetchPending)) void fetchDir(ws, path);
  }

  // A workspace can be opened by any client (this window, a future second
  // window, a remote backend): the store re-reads the list and, if nothing
  // is open yet, opens the first one — the boot rule, applied live.
  async function syncWorkspaces(): Promise<void> {
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


  // Paged read around the viewport (spec §8): the GUI decides the window,
  // the core serves the slice. Views keep the file's own id — the snapshot
  // and this paged read must share one id namespace or their copies of an
  // entry never match.
  //
  // In-flight dedupe: the transcript's window effect re-fires on every
  // window recompute (the post-open height settling is a storm of them),
  // and an identical (start, count) already in flight is shared, not
  // re-issued — so the recompute storm cannot re-request the same page.
  const windowsInFlight = new Map<string, Promise<void>>();

  // In-flight blob reads keyed by workspace + hash: the window effect's
  // recompute storm can hit the same blob twice; an identical read is
  // shared, not re-issued (the windowsInFlight pattern).
  const blobsInFlight = new Map<string, Promise<unknown>>();

  export async function fetchWindow(sid: string, start: number, count: number): Promise<void> {
    const s = store.sessions[sid];
    if (!s || count <= 0) return;
    const key = `${sid}:${start}:${count}`;
    const inFlight = windowsInFlight.get(key);
    if (inFlight) return inFlight;
    const done = (async () => {
      try {
        const out = await command({
          type: 'session_entries',
          session: sid,
          since: null,
          range: { start, count }
        });
        // A re-open can replace the session with a fresh state while this read is in flight (and the
        // re-open's identical window request is deduped onto it), so
        // merging into the object captured at issue time would land the
        // page in the discarded state and leave the re-opened transcript's
        // window unhydrated.
        const cur = store.sessions[sid];
        if (!cur) return;
        if (out.kind !== 'entries') return;
        const views = await resolveBlobs(out.entries, (v) => {
          const key = `${cur.meta.workspace}:${v.blob!.hash}`;
          const inflight = blobsInFlight.get(key);
          if (inflight) return inflight;
          const read = command({
            type: 'blob_read',
            workspace: cur.meta.workspace,
            id: v.blob!.id,
            hash: v.blob!.hash
          })
            .then((o) => (o.kind === 'blob' ? o.payload : null))
            .finally(() => blobsInFlight.delete(key));
          blobsInFlight.set(key, read);
          return read;
        });
        for (const v of views) upsertEntry(cur.entries, v);
      } finally {
        windowsInFlight.delete(key);
      }
    })();
    windowsInFlight.set(key, done);
    return done;
  }

  export async function send(text: string, lane: PendingMsg['lane']): Promise<void> {
    const sid = store.current;
    if (sid === null || text.trim() === '') return;
    const s = sessionOf(sid);
    s.pending = s.pending.filter((p) => !(p.text === text && p.lane === lane));
    // No optimistic card (ADR-0008): the core mints the entry id and the
    // card lands on the send's entry_upsert echo (~ms away on the local
    // channel); a rejected send simply never echoes.
    store.tailJump++;
    try {
      clearError();
      await command({
        type: 'message_send',
        session: sid,
        text,
        lane: lane === 'follow-up' ? 'follow_up' : lane
      });
      // The turn is dispatched: it is now 'starting' — waiting for the model's
      // first output (prefill + request latency). stream_start / a tool upsert
      // it to running; the waiting indicator and the stop button key off this.
      if (s.turn === 'idle') s.turn = 'starting';
    } catch (e) {
      store.error = errText(e);
    }
  }

  export async function stop(): Promise<void> {
    const sid = store.current;
    if (sid === null) return;
    try {
      clearError();
      await command({ type: 'message_stop', session: sid });
    } catch (e) {
      store.error = errText(e);
    }
  }

  export async function deleteQueueItem(
    text: string,
    lane: PendingMsg['lane'],
    source: string | null,
    idx: number
  ): Promise<void> {
    const sid = store.current;
    if (sid === null) return;
    const s = sessionOf(sid);
    // Duplicates are keyed by occurrence (text + lane + source); delete only
    // the idx-th of them. `pending` is the queue's global order, so the
    // occurrence's position in it is the global index the core deletes by.
    let seen = 0;
    let global = -1;
    for (let i = 0; i < s.pending.length; i++) {
      const p = s.pending[i]!;
      if (p.text === text && p.lane === lane && (p.source ?? null) === source) {
        if (seen === idx) {
          global = i;
          break;
        }
        seen++;
      }
    }
    if (global === -1) return;
    s.pending = s.pending.filter((_, i) => i !== global);
    try {
      clearError();
      await command({ type: 'queue_item_remove', session: sid, index: global });
    } catch {
      // optimistic: a queue event will resync (a concurrent turn boundary
      // may have consumed the item, in which case the resync is the truth)
    }
  }

  export function applyEvents(evts: Event[]): void {
    for (const ev of evts) {
      // Session-less events (the system group, skill_list_changed) carry
      // no `session` key at all — the presence check narrows the union.
      const sid = 'session' in ev ? ev.session : null;
      if (!sid) {
        if (ev.type === 'system') {
          // Only an error kind writes: an unrelated system event (from any
          // client) must not wipe a live IPC error (S3).
          if (ev.kind.kind === 'error') store.error = ev.kind.message;
          if (ev.kind.kind === 'workspace_opened') void syncWorkspaces();
        }
        if (ev.type === 'skill_list_changed') {
          // Full-state replacement (ticket #31): the change guard keeps an
          // unchanged re-emit a no-op.
          if (JSON.stringify(store.skills[ev.workspace] ?? []) !== JSON.stringify(ev.skills)) {
            store.skills[ev.workspace] = ev.skills;
          }
        }
        if (ev.type === 'file_tree_changed') {
          scheduleRefetch(ev.workspace, ev.changed);
          void refreshGitStatus(ev.workspace);
          // Live file-content view: a change under the active file's dir
          // (the watcher reports the parent dir) re-fetches its content + base
          // so the viewer tracks the file as it is edited.
          const af = store.activeFile;
          if (af && af.workspace === ev.workspace) {
            const touches = ev.changed.some((d) => af.path === d || af.path.startsWith(d + '/'));
            if (touches) void fetchActiveFile(af.workspace, af.path);
          }
        }
        continue;
      }
      const s = store.sessions[sid];
      if (!s) continue;
      switch (ev.type) {
        case 'stream_start': {
          const startedAt = Date.now();
          callStartMs.set(ev.call_id, startedAt);
          streamStartMs.set(ev.session, startedAt);
          s.tps = 0;
          s.turn = 'running';
          break;
        }
        case 'stream_end': {
          if (ev.usage) {
            s.usage = ev.usage;
            const started = callStartMs.get(ev.call_id);
            // #224: prefer the server's decode-window duration (from the
            // created/completed timestamps); it excludes prefill + transport,
            // which the wall-clock estimate wrongly includes.
            const secs = ev.duration_secs ?? (started !== undefined ? (Date.now() - started) / 1000 : null);
            if (secs !== null && secs > 0.2) s.tps = ev.usage.output_tokens / secs;
            callStartMs.delete(ev.call_id);
          }
          streamStartMs.delete(ev.session);
          // No tail re-read: the finalized lines already landed as upserts.
          s.turn = 'idle';
          break;
        }
        case 'entry_upsert': {
          upsertEntry(s.entries, ev.entry);
          // A tool call is the model's first output: leave 'starting'.
          if (s.turn === 'starting' && ev.entry.kind === 'tool') s.turn = 'running';
          // Live TPS: the exact output count lands only at stream_end, so
          // while the assistant streams, track a text-length estimate over
          // the elapsed stream time (the precise value replaces it at
          // stream_end).
          const streamStart = streamStartMs.get(ev.session);
          if (
            ev.entry.kind === 'assistant' &&
            (s.turn === 'running' || s.turn === 'starting') &&
            streamStart !== undefined
          ) {
            const secs = (Date.now() - streamStart) / 1000;
            const text = ((ev.entry.payload as { text?: string } | null) ?? {}).text ?? '';
            if (secs > 0.2) s.tps = Math.max(1, Math.round(text.length / 4)) / secs;
          }
          break;
        }
        case 'queue': {
          s.pending = ev.items.map((q) => ({
            text: q.text,
            lane: laneOf(q.lane),
            source: q.source ?? undefined
          }));
          break;
        }
        case 'session_event': {
          if (ev.kind.kind === 'branch_move') s.meta.leaf = ev.kind.leaf;
          break;
        }
        case 'subagent_event': {
          // Idempotent-cumulative (spec §8): each event carries the full
          // state of one handle; a lost batch self-heals on the next
          // snapshot.
          store.sessions = applySubagentEvent(store.sessions, ev.session, ev.kind, Date.now());
          break;
        }
        case 'om_status': {
          // The status bar's om gauge: the activity kind is event-driven;
          // the gauge values ride the snapshots.
          s.om.kind = ev.kind;
          break;
        }
        case 'plan_changed': {
          // The plan pane's data (spec §5.3): full-state replacement — the
          // watcher's parse renders in place; idempotent by construction.
          s.plan = ev.plan;
          break;
        }
        case 'system': {
          // Only an error kind writes (S3): the next successful command
          // clears the banner, not an unrelated system event.
          if (ev.kind.kind === 'error') {
            store.error = ev.kind.message;
            // A turn that failed before its first output (a provider 4xx, a
            // no output event to move the turn out of 'starting'.
            if (s.turn === 'starting') s.turn = 'idle';
          }
          break;
        }
      }
    }
  }
</script>
