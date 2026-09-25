<script module lang="ts">
  // App state for the central column (ticket #25). $state lives in a
  // .svelte module file because runes only transform there; components
  // import the exported `store` binding.
  //
  // The GUI is a stateless renderer (ADR-0006): every mutation is a
  // command round-trip or an event application, so a rebuild from
  // snapshot + events always converges. The snapshot is a metadata
  // skeleton (no payloads); paged reads around the viewport fill the
  // cards in (spec §8).

  import { command, isTauri, type Event, type SessionMeta, type SkillInfo, type SubagentInfo, type Workspace } from './protocol';
  import { decodeEntry, type PendingDelta, applyStreamEvent, applyToolEvent, mergeHydrated } from './entries';
  import {
    applySessionList,
    laneOf,
    openSession,
    type PendingMsg,
    type SessionState,
    setArchived,
    touchChild
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
  import { listen } from '@tauri-apps/api/event';
  import { open as pickDirectory } from '@tauri-apps/plugin-dialog';
  export const store = $state({
    focus: false,
    modelMenuOpen: false,
    reasoningOpen: false,
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
    // Per-tab isolation (spec §9): each open workspace owns its pane view
    // state; the transcript's conversation state stays per-session.
    pane: {} as Record<string, PaneState>
  });

  export interface PaneState {
    ltab: 'files' | 'sessions';
    rtab: 'tasks' | 'subs';
    // F3: the right-pane history section (done/failed/stopped rows) starts
    // collapsed; per-row badges carry the exact state, so no filter exists.
    historyOpen: boolean;
    expandedTasks: string[];
    // null = the default view (the group containing the active session
    // expanded); an array = the explicit set the user has toggled.
    openGroups: string[] | null;
    renamingId: string | null;
    archOpen: boolean;
    selSub: string | null;
    // The session-tree multiselect (cmd/ctrl toggle, shift range): the
    // selected ids and the row a shift range extends from.
    selected: string[];
    selAnchor: string | null;
  }

  // Pure read — a $derived may call this; creation goes through ensurePane
  // (a mutation, only from effects/actions).
  export function pane(ws: string | null): PaneState | null {
    if (ws === null) return null;
    return store.pane[ws] ?? null;
  }

  // Create-if-missing (the old pane() behavior) — call from effects/actions.
  export function ensurePane(ws: string): PaneState {
    const prev = store.pane[ws];
    if (prev) return prev;
    // The filters default to 'all': a done sub-agent or task that the user
    // just watched finish is what they expect to see, not an empty tab.
    const p: PaneState = {
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
    store.pane[ws] = p;
    return p;
  }

  export function toggleAllReasoning(): void {
    store.reasoningOpen = !store.reasoningOpen;
  }

  export function windowTitle(): string {
    const sid = store.current;
    const s = sid ? store.sessions[sid] : null;
    if (!s) return 'tau';
    const ws = store.workspaces.find((w) => w.id === s.meta.workspace);
    const wsName = ws ? ws.name : '';
    const name = s.meta.title ?? s.meta.id;
    const parent = s.parent ? store.sessions[s.parent] : null;
    if (!parent) return wsName ? `${wsName} · ${name}` : name;
    const p = parent.meta.title ?? parent.meta.id;
    return wsName ? `${wsName} · ${p} › ${name}` : `${p} › ${name}`;
  }

  // Deltas that land before their stream_start (a GUI connecting mid-stream):
  // buffered per call until the start or end arrives.
  let pendingDeltas = new Map<string, PendingDelta>();
  // Per-call clock (ms epoch) by call_id: TPS = a call's output tokens over
  // its own stream duration, shown in the status bar.
  let callStartMs = new Map<string, number>();

  function sessionOf(sid: string): SessionState {
    const s = store.sessions[sid];
    if (!s) throw new Error(`unknown session ${sid}`);
    return s;
  }

  // Rejections can be plain objects (a serialized core error) — String()
  // of one is "[object Object]".
  function errText(e: unknown): string {
    if (e instanceof Error) return e.message;
    if (typeof e === 'string') return e;
    if (e && typeof e === 'object') {
      const o = e as { message?: unknown; label?: unknown };
      if (typeof o.message === 'string') return o.message;
      if (typeof o.label === 'string') return o.label;
      try {
        return JSON.stringify(e);
      } catch {
        return String(e);
      }
    }
    return String(e);
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
      void listen('open_folder_requested', async () => {
        try {
          const picked = await pickDirectory({ directory: true, multiple: false });
          const dir = Array.isArray(picked) ? picked[0] : picked;
          if (typeof dir !== 'string' || dir === '') return;
          const name = dir.split(/[\\/]/).filter(Boolean).pop() ?? dir;
          openWorkspace({ id: '', name, cwd: dir });
        } catch (e) {
          store.error = errText(e);
        }
      });
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
    liveTexts: () =>
      (store.current ? store.sessions[store.current]?.live ?? [] : []).map((l) => l.text.length),
    // send/stop/openWorkspace/switchSession/fetchWindow are module exports
    // the rig drives directly; hoisted above.
    send,
    stop,
    openWorkspace,
    switchSession,
    fetchWindow,
    omStatus: (kind: 'observing' | 'reflecting' | 'idle') =>
      applyEvents([
        {
          type: 'om_status',
          workspace: store.current ? store.sessions[store.current].meta.workspace : '',
          session: store.current ?? '',
          kind
        }
      ]),
    sessionSetModel: (session: string, model: string) => command({ type: 'session_set_model', session, model })
  };


  // In-flight marker: the open's own workspace_opened event re-enters the
  // store via syncWorkspaces; without the marker it re-opens mid-flight,
  // and on an empty workspace the two session_new calls create a phantom
  // duplicate session.
  let opening = false;

  export async function openWorkspace(ws: Workspace): Promise<void> {
    if (opening) return;
    opening = true;
    try {
      await openWorkspaceInner(ws);
    } catch (e) {
      store.error = errText(e);
    } finally {
      opening = false;
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
    const skillsOut = await command({ type: 'skill_list', workspace: real.id });
    if (skillsOut.kind === 'skills') store.skills[real.id] = skillsOut.skills;
    const i = store.workspaces.findIndex((w) => w.cwd === real.cwd);
    if (i >= 0) {
      store.workspaces[i] = real;
    } else {
      store.workspaces.push(real);
    }
    const list = await command({ type: 'session_list', workspace: real.id });
    // The listed sessions materialize as stubs so the pane shows them all;
    // entries hydrate lazily when one is opened.
    if (list.kind === 'sessions') store.sessions = applySessionList(store.sessions, list.sessions);
    // The pane's first listing (ticket #32): the root, listed on open —
    // every other dir is fetched on expansion.
    store.files = ensureWorkspace(store.files, real.id);
    fetchDir(real.id, '.');
    const sid =
      list.kind === 'sessions' && list.sessions.length > 0
        ? list.sessions[0].id
        : ((await command({
            type: 'session_new',
            workspace: real.id,
            title: null
          })) as { kind: 'session'; session: SessionMeta }).session.id;
    await switchSession(sid);
  }

  // One directory's listing into the store. The change guard keeps an
  // unchanged refetch a no-op (no reactive churn, no re-render).
  async function fetchDir(ws: string, path: string): Promise<void> {
    let out;
    try {
      out = await command({ type: 'file_list', workspace: ws, path });
    } catch {
      // The workspace may be closing mid-flight: a fetch for a gone
      // workspace is dropped, not an error.
      return;
    }
    if (out.kind !== 'files') return;
    store.files = putListing(store.files, ws, path, out.files);
  }

  // Expansion: listed dirs collapse back (drop the fetch); unlisted dirs
  // fetch on first expand. A dir is listed only while expanded, so the
  // tree's memory tracks what is on screen.
  export function toggleFileDir(ws: string, path: string): void {
    const t = toggleDir(store.files, ws, path);
    store.files = t.cache;
    if (t.fetch) void fetchDir(ws, t.fetch);
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
    if (opening) return;
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
  export async function closeWorkspace(ws: Workspace): Promise<void> {
    const list = await command({ type: 'session_list', workspace: ws.id }).catch(() => ({
      kind: 'sessions' as const,
      sessions: []
    }));
    if (list.kind === 'sessions') {
      for (const s of list.sessions) {
        await command({ type: 'session_close', session: s.id }).catch(() => {});
      }
    }
    delete store.skills[ws.id];
    store.files = removeWorkspace(store.files, ws.id);
    store.workspaces = store.workspaces.filter((w) => w.id !== ws.id);
    for (const [sid, s] of Object.entries(store.sessions)) {
      if (s.meta.workspace === ws.id) delete store.sessions[sid];
    }
    pendingDeltas.clear();
    const cur = store.current;
    if (cur && store.sessions[cur]?.meta.workspace === ws.id) {
      store.current = null;
    }
  }

  // New top-level session in the workspace: the core names it (adjective-noun)
  // when no title is given. Opens it and drops straight into inline rename so
  // the generated name becomes a real one if the user cares. ⌘N (App.svelte)
  // and the sessions tab's ghost row both call this.
  export async function newSession(ws: string, title: string | null = null): Promise<string | null> {
    let sid: string;
    try {
      clearError();
      const out = (await command({
        type: 'session_new',
        workspace: ws,
        title
      })) as { kind: 'session'; session: SessionMeta };
      sid = out.session.id;
    } catch (e) {
      store.error = errText(e);
      return null;
    }
    await switchSession(sid);
    const q = pane(ws);
    if (q) q.renamingId = sid;
    return sid;
  }

  export async function renameSession(sid: string, title: string): Promise<void> {
    const t = title.trim();
    if (!t) return;
    try {
      clearError();
      await command({ type: 'session_rename', session: sid, title: t });
    } catch (e) {
      store.error = errText(e);
      return;
    }
    const s = store.sessions[sid];
    if (s) s.meta.title = t;
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
  // Archive (ADR-0005): one-way, off the live read/write path. The core
  // stops a running child first and archives the session's children with
  // it; the row keeps its state (a message resumes it) and moves to the
  // archive folder, the children's rows converge on the refetched list.
  export async function archiveSession(sid: string): Promise<void> {
    // A child is archived by its parent's cascade, never directly — a
    // standalone child archive would leave it out of the parent's row in
    // the archive folder.
    if (store.sessions[sid]?.meta.parent) return;
    try {
      clearError();
      const out = await command({ type: 'session_archive', session: sid });
      if (out.kind === 'session') {
        store.sessions = setArchived(store.sessions, sid, out.session);
      }
    } catch (e) {
      store.error = errText(e);
      return;
    }
    const wsid = store.sessions[sid]?.meta.workspace;
    if (wsid) {
      try {
        const list = await command({ type: 'session_list', workspace: wsid });
        if (list.kind === 'sessions') store.sessions = applySessionList(store.sessions, list.sessions);
      } catch (e) {
        store.error = errText(e);
      }
    }
  }

  // Restore (ADR-0005): the file moves back from the archive dir (the core
  // restores a parent's children with it); the rows converge on the
  // refetched list, the archive flag's authority.
  export async function restoreSession(ws: string, sid: string): Promise<void> {
    try {
      clearError();
      await command({ type: 'session_restore', workspace: ws, session: sid });
    } catch (e) {
      store.error = errText(e);
      return;
    }
    try {
      const list = await command({ type: 'session_list', workspace: ws });
      if (list.kind === 'sessions') store.sessions = applySessionList(store.sessions, list.sessions);
    } catch (e) {
      store.error = errText(e);
    }
  }

  export async function switchSession(sid: string): Promise<void> {
    const prev = store.current;
    store.current = sid;
    try {
      clearError();
      const out = await command({ type: 'session_open', session: sid });
      if (out.kind !== 'snapshot') throw new Error('unexpected session_open output');
      store.sessions = openSession(store.sessions, sid, out.snapshot);
    } catch (e) {
      // A failed open must not leave current dangling at an unhydrated
      // stub: roll back to the previous session and surface the error.
      store.error = errText(e);
      store.current = prev;
    }
  }

  // A pane action (session tree row / sub-agent double-click): the child is
  // an ordinary session — opening it switches the current view.
  export async function openSessionById(sid: string): Promise<void> {
    await switchSession(sid);
  }

  // Paged read around the viewport (spec §8): the GUI decides the window,
  // the core serves the slice. Views keep the file's own id — the snapshot
  // and this paged read must share one id namespace or their copies of an
  // entry never match.
  export async function fetchWindow(sid: string, start: number, count: number): Promise<void> {
    const s = store.sessions[sid];
    if (!s || count <= 0) return;
    const out = await command({
      type: 'session_entries',
      session: sid,
      since: null,
      range: { start, count }
    });
    if (out.kind !== 'entries') return;
    const m = mergeHydrated(s.entries, s.live, out.entries);
    s.entries = m.entries;
    s.live = m.live;
  }

  export async function send(text: string, lane: PendingMsg['lane']): Promise<void> {
    const sid = store.current;
    if (sid === null || text.trim() === '') return;
    const s = sessionOf(sid);
    s.pending = s.pending.filter((p) => !(p.text === text && p.lane === lane));
  // The user bubble appears at send time; the core's file copy of the
  // same entry hydrates later and is dropped against this one (twin).
  // A rejected send never reaches the core, so its twin never arrives —
  // the optimistic entry is spliced back out instead of ghosting.
  const optimisticId = `u-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  s.entries.push({ id: optimisticId, kind: 'user', text });
  store.tailJump++;
  try {
    clearError();
    await command({
      type: 'message_send',
      session: sid,
      text,
      lane: lane === 'follow-up' ? 'follow_up' : lane
    });
  } catch (e) {
    const i = s.entries.findIndex((x) => x.id === optimisticId);
    if (i >= 0) s.entries.splice(i, 1);
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
    idx: number
  ): Promise<void> {
    const sid = store.current;
    if (sid === null) return;
    const s = sessionOf(sid);
    // Duplicates are keyed by occurrence; delete only the idx-th of them.
    let seen = 0;
    s.pending = s.pending.filter((p) => {
      if (p.text !== text || p.lane !== lane) return true;
      return seen++ !== idx;
    });
    try {
      clearError();
      const out = await command({ type: 'session_open', session: sid });
      if (out.kind === 'snapshot') {
        s.pending = out.snapshot.live.queue.map((q) => ({ text: q.text, lane: laneOf(q.lane) }));
      }
    } catch {
      // optimistic: the queue event will resync
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
          // Full-state replacement (ticket #31, the task_changed pattern):
          // the change guard keeps an unchanged re-emit a no-op.
          if (JSON.stringify(store.skills[ev.workspace] ?? []) !== JSON.stringify(ev.skills)) {
            store.skills[ev.workspace] = ev.skills;
          }
        }
        if (ev.type === 'file_tree_changed') {
          scheduleRefetch(ev.workspace, ev.changed);
        }
        continue;
      }
      const s = store.sessions[sid];
      if (!s) continue;
      switch (ev.type) {
        case 'stream_start': {
          s.live = applyStreamEvent(ev, s.entries, s.live, pendingDeltas).live;
          callStartMs.set(ev.call_id, Date.now());
          s.tps = 0;
          s.turn = 'running';
          break;
        }
        case 'stream_delta': {
          s.live = applyStreamEvent(ev, s.entries, s.live, pendingDeltas).live;
          break;
        }
        case 'stream_end': {
          const m = applyStreamEvent(ev, s.entries, s.live, pendingDeltas);
          s.entries = m.entries;
          s.live = m.live;
          if (ev.usage) {
            s.usage = ev.usage;
            const started = callStartMs.get(ev.call_id);
            if (started !== undefined) {
              const secs = (Date.now() - started) / 1000;
              if (secs > 0.2) s.tps = ev.usage.output_tokens / secs;
            }
            callStartMs.delete(ev.call_id);
          }
          if (s.live.length === 0) {
            s.turn = 'idle';
            // Post-turn reconciliation (GUI side): the session file is the
            // record — re-read the tail so tool entries and the final
            // assistant state land even if their events raced the stream.
            void fetchWindow(sid, Math.max(0, s.entries.length - 50), 50);
          }
          s.meta.leaf = m.fin?.id ?? s.entries[s.entries.length - 1]?.id ?? s.meta.leaf;
          break;
        }
        case 'tool_start': {
          const m = applyToolEvent(ev, s.entries, s.live);
          s.entries = m.entries;
          s.live = m.live;
          break;
        }
        case 'tool_end': {
          s.entries = applyToolEvent(ev, s.entries, s.live).entries;
          break;
        }
        case 'queue': {
          s.pending = ev.items.map((q) => ({ text: q.text, lane: laneOf(q.lane) }));
          break;
        }
        case 'task_changed': {
          // Full-state replacement (idempotent, spec §8): one event covers
          // both the model's task tools and the app's task commands.
          if (JSON.stringify(s.tasks) !== JSON.stringify(ev.tasks)) s.tasks = ev.tasks;
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
          const k = ev.kind;
          const now = Date.now();
          if (k.kind === 'spawned') {
            const info: SubagentInfo = {
              handle: k.handle,
              child: k.child,
              agent_type: k.agent_type,
              context_mode: k.context_mode,
              state: 'running',
              waiting_on: null,
              last_message: null,
              usage: null,
              task: null,
              resume_contract: null
            };
            s.subagents = s.subagents.filter((x) => x.handle !== k.handle && x.child !== k.child);
            s.subagents.push(info);
            store.sessions = touchChild(store.sessions, ev.session, k.child, 'running', now, null, k.title);
            break;
          }
          if (k.kind === 'state') {
            const st = k.state as SessionState['state'];
            const info = s.subagents.find((x) => x.handle === k.handle);
            // The live bridge sends detail as an object (core.rs: {waiting_on}
            // for idle, {by, resume_contract?} for stopped, {output},
            // {reason}, null for running).
            const d = k.detail as { waiting_on?: unknown } | null;
            const waitingOn =
              d && typeof d === 'object' && typeof d.waiting_on === 'string'
                ? d.waiting_on
                : null;
            if (info) {
              info.state = st;
              if (waitingOn !== null) info.waiting_on = waitingOn;
              else if (st !== 'idle') info.waiting_on = null;
              if (k.note) info.last_message = k.note;
            }
            store.sessions = touchChild(store.sessions, ev.session, k.child, st, now, waitingOn);
            break;
          }
          // notified: a child notification reached the parent. The wake
          // kind maps onto a terminal state (done/failed/stopped) — a
          // stopped child must not read back as idle.
          const st =
            k.wake === 'done' ? 'done' : k.wake === 'failed' ? 'failed' : k.wake === 'stopped' ? 'stopped' : 'idle';
          const child = store.sessions[k.child];
          if (child) child.mru = now;
          const info = s.subagents.find((x) => x.child === k.child);
          if (info) {
            info.last_message = k.text;
            info.state = st;
            if (st === 'idle' && info.waiting_on === null) info.waiting_on = 'parent';
            if (st !== 'idle') info.waiting_on = null;
          }
          if (child) {
            child.state = st;
            if (st === 'idle' && child.waiting_on === null) child.waiting_on = 'parent';
            if (st !== 'idle') child.waiting_on = null;
          }
          break;
        }
        case 'om_status': {
          // The status bar's om gauge: the activity kind is event-driven;
          // the gauge values ride the snapshots.
          s.om.kind = ev.kind;
          break;
        }
        case 'system': {
          // Only an error kind writes (S3): the next successful command
          // clears the banner, not an unrelated system event.
          if (ev.kind.kind === 'error') store.error = ev.kind.message;
          break;
        }
      }
    }
  }
</script>
