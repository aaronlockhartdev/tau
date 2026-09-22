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

  import {
    command,
    isTauri,
    type Entry,
    type Event,
    type FileEntry,
    type QueuedItem,
    type SessionMeta,
    type SkillInfo,
    type Snapshot,
    type SubagentInfo,
    type Task,
    type Usage,
    type ViewEntry,
    type Workspace
  } from './protocol';
  import { toEntry } from './fixture';
  import { listen } from '@tauri-apps/api/event';
  import { open as pickDirectory } from '@tauri-apps/plugin-dialog';


  export interface PendingMsg {
    text: string;
    lane: 'force' | 'steering' | 'follow-up';
  }

  export interface SessionState {
    meta: SessionMeta;
    entries: Entry[];
    live: Entry[];
    usage: Usage | null;
    // Output tokens/second of the session's most recent turn (status bar).
    tps: number;
    turn: 'running' | 'idle';
    pending: PendingMsg[];
    // The session-tree row (ticket #26): a child session's parent link,
    // its lifecycle state, its archive flag, and its sort key.
    parent: string | null;
    state: 'running' | 'idle' | 'done' | 'failed' | 'stopped';
    // The child's declared wait (parent | user | subagent) — from the state
    // event's detail; annotated on the row and the child's own header.
    waiting_on: string | null;
    archived: boolean;
    mru: number;
    // The session's children (the parent's view — the sub-agent panel) and
    // its tasks (the tasks panel; active ones carry their resume contract).
    subagents: SubagentInfo[];
    tasks: Task[];
  }

  export const store = $state({
    focus: false,
    // 'r' keybind (App.svelte): every reasoning block opens/closes at once.
    reasoningOpen: false,
    workspaces: [] as Workspace[],
    current: null as string | null,
    // Bumped by send: the transcript jumps to the new user entry and follows
    // the turn, so blocks appear while they are written.
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
    files: {} as Record<string, Record<string, FileEntry[]>>,
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
    // The session row being renamed (double-click), per pane.
    renamingId: string | null;
    archOpen: boolean;
    selSub: string | null;
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
      selSub: null
    };
    store.pane[ws] = p;
    return p;
  }

  export function toggleAllReasoning(): void {
    store.reasoningOpen = !store.reasoningOpen;
  }

  // A spawn/state event for a child we haven't opened yet: register a stub
  // so the session tree can group it; a later session_open replaces the
  // stub with the real snapshot (keeping the parent link, which the child's
  // own snapshot does not carry).
  function touchChild(parentSid: string, childSid: string, state: SessionState['state'], mru: number, waitingOn: string | null = null, title: string | null = null): void {
    const parent = store.sessions[parentSid];
    const ws = parent?.meta.workspace ?? '';
    const prev = store.sessions[childSid];
    if (prev) {
      prev.state = state;
      prev.mru = mru;
      if (waitingOn !== null) prev.waiting_on = waitingOn;
      // A late title (the spawn event) fills the stub's blank name; a real
      // name already set (from a snapshot) wins.
      if (title !== null && prev.meta.title === null) prev.meta.title = title;
      return;
    }
    store.sessions[childSid] = {
      meta: { id: childSid, workspace: ws, title, parent: parentSid, created: mru, leaf: null, model: null, usage: null, archived: false },
      entries: [],
      live: [],
      usage: null,
      tps: 0,
      turn: state === 'running' ? 'running' : 'idle',
      pending: [],
      parent: parentSid,
      state,
      waiting_on: state === 'idle' ? waitingOn : null,
      archived: false,
      mru,
      subagents: [],
      tasks: []
    };
  }

  // Deltas that land before their stream_start (a GUI connecting mid-stream):
  // buffered per call until the start or end arrives.
  let pendingDeltas = new Map<string, { text: string; reasoning: string }>();
  // Per-call clock (ms epoch) by call_id: TPS = a call's output tokens over
  // its own stream duration, shown in the status bar.
  let callStartMs = new Map<string, number>();
  const PENDING_CAP = 64 * 1024;


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

  function laneOf(l: QueuedItem['lane']): PendingMsg['lane'] {
    return l === 'follow_up' ? 'follow-up' : l;
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
      (store.current ? store.sessions[store.current]?.live ?? [] : []).map((l) => (l.text ?? '').length),
    // send/stop/openWorkspace are module exports the rig drives directly;
    // hoisted above.
    send,
    stop,
    openWorkspace
  };


  function snapshotToState(snap: Snapshot): SessionState {
    const meta = snap.session;
    return {
      meta,
      // metadata skeleton: the card shows the preview until a paged read
      // replaces it with the payload.
      entries: snap.entries.map((m) => ({
        id: m.id,
        kind:
          m.kind === 'assistant'
            ? m.status === 'interrupted'
              ? 'interrupted'
              : 'message'
            : m.kind,
        text: m.preview
      })),
      live: [],
      usage: meta.usage,
      turn: snap.live.turn === 'running' ? 'running' : 'idle',
      tps: 0,
      pending: snap.live.queue.map((q) => ({ text: q.text, lane: laneOf(q.lane) })),
      parent: null,
      state: snap.live.turn === 'running' ? 'running' : 'idle',
      waiting_on: null,
      archived: meta.archived,
      mru: meta.created,
      subagents: snap.live.subagents,
      tasks: snap.live.tasks
    };
  }


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
    if (list.kind === 'sessions') applySessionList(list.sessions);
    // The pane's first listing (ticket #32): the root, listed on open —
    // every other dir is fetched on expansion.
    store.files[real.id] ??= {};
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

  // --- The files pane (ticket #32): listed dirs, lazy expansion, invalidation.

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
    const cur = store.files[ws];
    if (!cur) return;
    if (JSON.stringify(cur[path] ?? []) === JSON.stringify(out.files)) return;
    cur[path] = out.files;
  }

  // Expansion: listed dirs collapse back (drop the fetch); unlisted dirs
  // fetch on first expand. A dir is listed only while expanded, so the
  // tree's memory tracks what is on screen.
  export function toggleFileDir(ws: string, path: string): void {
    const cur = store.files[ws];
    if (!cur) return;
    if (cur[path]) {
      delete cur[path];
      return;
    }
    cur[path] = [];
    void fetchDir(ws, path);
  }

  // Invalidation (the design's lost-events case, client side): a burst of
  // file_tree_changed events coalesces into one refetch wave — the 300 ms
  // timer collapses bursts, and only listed (expanded) dirs are refetched:
  // a change in an unlisted dir is fetched the moment the user expands it.
  const refetchPending = new Map<string, Set<string>>();
  let refetchTimer: ReturnType<typeof setTimeout> | null = null;

  function scheduleRefetch(ws: string, dirs: string[]): void {
    const cur = store.files[ws];
    if (!cur) return;
    let set = refetchPending.get(ws);
    if (!set) {
      set = new Set();
      refetchPending.set(ws, set);
    }
    for (const d of dirs) {
      if (cur[d]) set.add(d);
    }
    if (refetchTimer) clearTimeout(refetchTimer);
    refetchTimer = setTimeout(flushRefetch, 300);
  }

  function flushRefetch(): void {
    refetchTimer = null;
    for (const [ws, dirs] of refetchPending) {
      refetchPending.delete(ws);
      // A dir collapsed inside the 300 ms window is no longer listed: drop
      // it here, not at schedule time (the listed? guard must be fresh).
      const cur = store.files[ws];
      for (const d of dirs) if (cur?.[d]) void fetchDir(ws, d);
    }
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
    delete store.files[ws.id];
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

  export async function renameSession(sid: string, title: string): Promise<void> {
    const t = title.trim();
    if (!t) return;
    try {
      await command({ type: 'session_rename', session: sid, title: t });
    } catch (e) {
      store.error = errText(e);
      return;
    }
    const s = store.sessions[sid];
    if (s) s.meta.title = t;
  }

  // Archive (ADR-0005): one-way, off the live read/write path. The core
  // stops a running child first and archives the session's children with
  // it; the row keeps its state (a message resumes it) and moves to the
  // archive folder, the children's rows converge on the refetched list.
  export async function archiveSession(sid: string): Promise<void> {
    try {
      const out = await command({ type: 'session_archive', session: sid });
      if (out.kind === 'session') {
        const s = store.sessions[sid];
        if (s) {
          s.meta = out.session;
          s.archived = out.session.archived;
        }
      }
    } catch (e) {
      store.error = errText(e);
      return;
    }
    const wsid = store.sessions[sid]?.meta.workspace;
    if (wsid) {
      try {
        const list = await command({ type: 'session_list', workspace: wsid });
        if (list.kind === 'sessions') applySessionList(list.sessions);
      } catch (e) {
        store.error = errText(e);
      }
    }
  }

  // Merge a session_list into the store: an existing row adopts the list's
  // archive flag (the list is the flag's authority — a row archived while
  // listed, or listed after a restart, converges on the file's state); a
  // new id materializes as a stub (entries hydrate lazily on open).
  function applySessionList(sessions: SessionMeta[]): void {
    for (const m of sessions) {
      const existing = store.sessions[m.id];
      if (existing) {
        existing.archived = m.archived;
      } else {
        store.sessions[m.id] = {
          meta: m,
          entries: [],
          live: [],
          usage: null,
          tps: 0,
          turn: 'idle',
          pending: [],
          parent: m.parent ?? null,
          state: 'idle',
          waiting_on: null,
          archived: m.archived,
          mru: m.created,
          subagents: [],
          tasks: []
        };
      }
    }
  }

  // Restore (ADR-0005): the file moves back from the archive dir (the core
  // restores a parent's children with it); the rows converge on the
  // refetched list, the archive flag's authority.
  export async function restoreSession(ws: string, sid: string): Promise<void> {
    try {
      await command({ type: 'session_restore', workspace: ws, session: sid });
    } catch (e) {
      store.error = errText(e);
      return;
    }
    try {
      const list = await command({ type: 'session_list', workspace: ws });
      if (list.kind === 'sessions') applySessionList(list.sessions);
    } catch (e) {
      store.error = errText(e);
    }
  }

  export async function switchSession(sid: string): Promise<void> {
    store.current = sid;
    const out = await command({ type: 'session_open', session: sid });
    if (out.kind !== 'snapshot') throw new Error('unexpected session_open output');
    const next = snapshotToState(out.snapshot);
    // A child's parent link is not in its own snapshot (it lives in the
    // parent's sub-agent list); keep what the tree already knew.
    const prev = store.sessions[sid];
    if (prev) {
      next.parent = prev.parent;
      next.state = prev.state;
      next.archived = prev.archived;
    }
    store.sessions[sid] = next;
    // Self-heal the child stubs: the snapshot's sub-agent list is the
    // supervisor's ground truth, so it both fills a lost spawn and
    // corrects a stale stub (a done child must not keep its pre-wake
    // state). A corrected stub keeps its own mru, so the sort is stable.
    for (const sub of next.subagents) {
      const existing = store.sessions[sub.child];
      if (!existing) {
        touchChild(sid, sub.child, sub.state, Date.now(), sub.waiting_on, null);
      } else {
        touchChild(sid, sub.child, sub.state, existing.mru, sub.waiting_on, null);
        if (sub.waiting_on === null && sub.state !== 'idle') existing.waiting_on = null;
      }
    }
    // A disk-restored child has no live supervisor listing it: synthesize
    // the tab entry from the child stub so the sub-agents tab is never
    // empty; a real spawn/state event (matched by child) replaces it.
    for (const child of Object.values(store.sessions)) {
      if (child.parent !== sid || next.subagents.some((x) => x.child === child.meta.id)) continue;
      next.subagents.push({
        handle: child.meta.id,
        child: child.meta.id,
        agent_type: 'general',
        context_mode: 'fresh',
        state: child.state,
        waiting_on: child.waiting_on ?? null,
        last_message: null,
        usage: child.usage,
        task: null,
        resume_contract: null
      });
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
    mergeHydrated(s, out.entries);
  }

  // One logical entry appears under two id namespaces: streamed under its call_id / u-
  // prefix / provider tool_call_id, persisted under the file's counter. The
  // namespaces are not comparable, so the streamed slot is canonical: a file
  // entry whose twin is streamed (in entries, live mid-turn, or a duplicate
  // file copy from the snapshot) hydrates that slot in place and the file
  // copy is dropped. A file entry with no twin takes its own id and appends
  // in arrival order.
  function mergeHydrated(s: SessionState, views: ViewEntry[]): void {
    const isTwin = (e: { id: string; text?: string; reasoning?: string }, v: ViewEntry, next: Entry) => {
      // Streamed ids are non-numeric (call_id, u-…, the provider's
      // tool_call_id); a file-counter id is a snapshot copy, never a twin.
      if (/^\d+$/.test(e.id)) return false;
      if (next.kind === 'tool') {
        return e.id === String((v.payload as Record<string, unknown>).call_id ?? '');
      }
      // An empty-text assistant (reasoning-only) has no text to match on:
      // its reasoning is the identity — the streamed copy and the file copy
      // carry byte-identical reasoning.
      if (!next.text) return Boolean(next.reasoning) && e.reasoning === next.reasoning;
      return e.text === next.text;
    };
    const hydrate = (e: Entry, next: Entry): Entry => ({
      ...e,
      kind: next.kind,
      text: next.text,
      reasoning: next.reasoning,
      output: next.output,
      status: next.status,
      name: next.name,
      args: next.args,
      usage: next.usage,
      source: next.source
    });
    for (const v of views) {
      const next = toEntry(v);
      const i = s.entries.findIndex((e) => e.id === v.id);
      if (i >= 0) {
        // A file copy is already present (snapshot). If the streamed twin of
        // the same logical entry exists too, collapse: the streamed slot
        // keeps its position and id, adopts the file's payload, and the
        // file copy is removed — otherwise the entry renders twice.
        const ti = s.entries.findIndex((e, j) => j !== i && isTwin(e, v, next));
        if (ti >= 0) {
          // Assign only on a real change: an unconditional replace makes the
          // merge reactive every 25 ms fetch and the window effect re-enters
          // forever.
          const t = s.entries[ti];
          if (
            t.kind !== next.kind ||
            t.text !== next.text ||
            t.reasoning !== next.reasoning ||
            t.output !== next.output ||
            t.status !== next.status ||
            t.name !== next.name ||
            t.args !== next.args ||
            t.source !== next.source
          ) {
            s.entries[ti] = hydrate(t, next);
          }
          s.entries.splice(i, 1);
          continue;
        }
        const old = s.entries[i];
        if (
          old.kind !== next.kind ||
          old.text !== next.text ||
          old.reasoning !== next.reasoning ||
          old.output !== next.output ||
          old.status !== next.status ||
          old.name !== next.name ||
          old.args !== next.args ||
          old.source !== next.source
        ) {
          s.entries[i] = next;
        }
        continue;
      }
      const li = s.live.findIndex((e) => isTwin(e, v, next));
      if (li >= 0) {
        // Mid-turn: the live slot adopts the file's payload but keeps its
        // streamed id, so the promoted entry keeps its position. Guarded on
        // a real change — the 25 ms fetch re-enters while the turn runs and
        // an unconditional replace never converges.
        const l = s.live[li];
        if (l.text !== next.text || l.reasoning !== next.reasoning) {
          s.live[li] = { ...next, id: l.id };
        }
        continue;
      }
      if (s.entries.some((e) => isTwin(e, v, next))) continue;
      s.entries.push(next);
    }
  }

  export async function send(text: string, lane: PendingMsg['lane']): Promise<void> {
    const sid = store.current;
    if (sid === null || text.trim() === '') return;
    const s = sessionOf(sid);
    s.pending = s.pending.filter((p) => !(p.text === text && p.lane === lane));
    // The user bubble appears at send time; the core's file copy of the
    // same entry hydrates later and is dropped against this one (twin).
    s.entries.push({ id: `u-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`, kind: 'user', text });
    store.tailJump++;
    try {
      await command({
        type: 'message_send',
        session: sid,
        text,
        lane: lane === 'follow-up' ? 'follow_up' : lane
      });
    } catch (e) {
      store.error = errText(e);
    }
  }

  export async function stop(): Promise<void> {
    const sid = store.current;
    if (sid === null) return;
    try {
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
          store.error = ev.kind.kind === 'error' ? ev.kind.message : null;
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
          const le: Entry = { id: ev.call_id, kind: 'message', text: '', reasoning: '' };
          const pd = pendingDeltas.get(ev.call_id);
          if (pd) {
            le.text = pd.text;
            le.reasoning = pd.reasoning;
            pendingDeltas.delete(ev.call_id);
          }
          s.live.push(le);
          callStartMs.set(ev.call_id, Date.now());
          s.tps = 0;
          s.turn = 'running';
          break;
        }
        case 'stream_delta': {
          const le = s.live.find((x) => x.id === ev.call_id);
          if (le) {
            le.text += ev.text;
            if (ev.reasoning) le.reasoning += ev.reasoning;
          } else {
            const pd = pendingDeltas.get(ev.call_id) ?? { text: '', reasoning: '' };
            if (pd.text.length < PENDING_CAP) {
              pd.text += ev.text;
              if (ev.reasoning) pd.reasoning += ev.reasoning;
              pendingDeltas.set(ev.call_id, pd);
            }
          }
          break;
        }
        case 'stream_end': {
          let le = s.live.find((x) => x.id === ev.call_id);
          s.live = s.live.filter((x) => x.id !== ev.call_id);
          if (!le) {
            // Ended before we saw its start: close it out of the buffer.
            const pd = pendingDeltas.get(ev.call_id);
            if (pd) {
              pendingDeltas.delete(ev.call_id);
              le = { id: ev.call_id, kind: 'message', text: pd.text, reasoning: pd.reasoning };
            }
          }
          if (le) {
            const done: Entry = {
              id: le.id,
              kind: ev.interrupted ? 'interrupted' : 'message',
              text: le.text,
              reasoning: le.reasoning || undefined,
              usage: ev.usage ?? undefined
            };
            // The paged read can hydrate this entry's file copy during the
            // turn; that copy is canonical, so only push the streamed one
            // when no file twin exists.
            const dup = s.entries.some(
              (e) => /^\d+$/.test(e.id) && e.kind === done.kind && e.text === done.text
            );
            if (!dup) s.entries.push(done);
          }
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
          s.meta.leaf = le?.id ?? s.entries[s.entries.length - 1]?.id ?? s.meta.leaf;
          break;
        }
        // Tool entries key on the provider's tool_call_id (not the stream
        // call_id): that is the id persisted in the file payload, so the
        // hydration twin matches, and two tools in one assistant call no
        // longer collapse into one card.
        case 'tool_start': {
          // A tool call follows the assistant text that requested it: settle
          // the streaming entry into the list first so the card lands after
          // that text, not after the response that follows it.
          for (const le of s.live) {
            s.entries.push({
              id: le.id,
              kind: 'message',
              text: le.text,
              reasoning: le.reasoning || undefined
            });
          }
          s.live = [];
          const existing = s.entries.find((e) => e.id === ev.tool_call_id);
          if (existing) {
            existing.status = 'running';
          } else {
            // The end-of-turn pump can deliver this after a later turn's
            // entries have already landed: place the card right after the
            // assistant call that made it, not at the tail.
            const ai = s.entries.findIndex((x) => x.id === ev.call_id);
            if (ai >= 0) {
              s.entries.splice(ai + 1, 0, { id: ev.tool_call_id, kind: 'tool', name: ev.name, status: 'running' });
            } else {
              s.entries.push({ id: ev.tool_call_id, kind: 'tool', name: ev.name, status: 'running' });
            }
          }
          break;
        }
        case 'tool_end': {
          const e = s.entries.find((x) => x.id === ev.tool_call_id);
          if (e) {
            e.status = 'ok';
            e.output = ev.output === undefined ? undefined : String(ev.output);
          }
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
            touchChild(ev.session, k.child, 'running', now, null, k.title);
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
            touchChild(ev.session, k.child, st, now, waitingOn);
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
        case 'system': {
          store.error = ev.kind.kind === 'error' ? ev.kind.message : null;
          break;
        }
      }
    }
  }
</script>
