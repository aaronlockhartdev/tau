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
    type QueuedItem,
    type SessionMeta,
    type Snapshot,
    type SubagentInfo,
    type Task,
    type Usage,
    type ViewEntry,
    type Workspace
  } from './protocol';
  import { buildDemoSession, toEntry } from './fixture';

  export interface LiveEntry {
    id: string;
    text: string;
    reasoning: string;
  }

  export interface PendingMsg {
    text: string;
    lane: 'force' | 'steering' | 'follow-up';
  }

  export interface SessionState {
    meta: SessionMeta;
    entries: Entry[];
    live: LiveEntry[];
    usage: Usage | null;
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
    workspaces: [] as Workspace[],
    current: null as string | null,
    sessions: {} as Record<string, SessionState>,
    loading: false,
    error: null as string | null,
    demo: false,
    // The transcript's last window computation (spec §9 seg3 render stats).
    renderRange: '',
    renderMs: 0,
    // Per-tab isolation (spec §9): each open workspace owns its pane view
    // state; the transcript's conversation state stays per-session.
    pane: {} as Record<string, PaneState>
  });

  export interface PaneState {
    ltab: 'files' | 'sessions';
    rtab: 'tasks' | 'subs';
    tFilter: 'open' | 'all' | 'in-progress' | 'blocked' | 'pending' | 'done';
    sFilter: 'open' | 'all' | 'running' | 'idle' | 'failed' | 'stopped' | 'done';
    expandedTasks: string[];
    openGroups: string[];
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
    const p: PaneState = {
      ltab: 'sessions',
      rtab: 'tasks',
      tFilter: 'open',
      sFilter: 'open',
      expandedTasks: [],
      openGroups: [],
      archOpen: false,
      selSub: null
    };
    store.pane[ws] = p;
    return p;
  }

  // A spawn/state event for a child we haven't opened yet: register a stub
  // so the session tree can group it; a later session_open replaces the
  // stub with the real snapshot (keeping the parent link, which the child's
  // own snapshot does not carry).
  function touchChild(parentSid: string, childSid: string, state: SessionState['state'], mru: number): void {
    const parent = store.sessions[parentSid];
    const ws = parent?.meta.workspace ?? '';
    const prev = store.sessions[childSid];
    if (prev) {
      prev.state = state;
      prev.mru = mru;
      return;
    }
    store.sessions[childSid] = {
      meta: { id: childSid, workspace: ws, title: null, created: mru, leaf: null, model: null, usage: null },
      entries: [],
      live: [],
      usage: null,
      turn: state === 'running' ? 'running' : 'idle',
      pending: [],
      parent: parentSid,
      state,
      archived: false,
      mru,
      subagents: [],
      tasks: []
    };
  }

  // Deltas that land before their stream_start (a GUI connecting mid-stream):
  // buffered per call until the start or end arrives.
  let pendingDeltas = new Map<string, { text: string; reasoning: string }>();
  const PENDING_CAP = 64 * 1024;

  let demoViews: ViewEntry[] = [];
  let demoStreamsStarted = false;

  function sessionOf(sid: string): SessionState {
    const s = store.sessions[sid];
    if (!s) throw new Error(`unknown session ${sid}`);
    return s;
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
      if (params.get('demo') === '1') {
        startDemo();
        return;
      }
      // Outside the Tauri shell there is no core to talk to; invoke would
      // hang forever, so fail fast with the one way to run in a browser.
      if (!isTauri()) {
        store.error = 'no Tauri window — run the app, or open with ?demo=1 for the browser demo';
        return;
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
      store.error = e instanceof Error ? e.message : String(e);
    } finally {
      store.loading = false;
    }
  }

  function startDemo(): void {
    store.demo = true;
    const { meta, entries, views } = buildDemoSession();
    demoViews = views;
    store.workspaces = [{ id: 'w-demo', name: 'tau', cwd: '~/git/tau' }];
    store.current = meta.id;
    store.sessions[meta.id] = {
      meta,
      entries,
      live: [],
      usage: meta.usage,
      turn: 'idle',
      pending: [
        { text: 'use the 62-char alphabet, not base36', lane: 'steering' },
        { text: 'before you finish, run cargo fmt', lane: 'steering' },
        { text: 'also update CONTEXT.md with the new terms', lane: 'follow-up' }
      ],
      parent: null,
      state: 'idle',
      waiting_on: null,
      archived: false,
      mru: meta.created,
      subagents: demoSubagents(),
      tasks: demoTasks()
    };
    // The demo parent's usage: the sum of its children's (the bar's usage
    // segment shows real tokens, not undefined).
    store.sessions[meta.id].usage = { input_tokens: 195800, output_tokens: 62200, total_tokens: 258000 };
    for (const c of demoChildren()) store.sessions[c.meta.id] = c;
    startDemoStreams();
  }

  // The #26 pane dataset (the prototype's): five sub-agent sessions under
  // the demo session — one per lifecycle state — plus a nested pair under
  // the idle one, and the five tasks with steps/criteria/evidence.
  function demoMeta(id: string, title: string, created: number): SessionMeta {
    return {
      id,
      workspace: 'w-demo',
      title,
      created,
      leaf: null,
      model: 'vllm/qwen3.8-27b',
      usage: null
    };
  }

  function demoChild(
    id: string,
    title: string,
    handle: string,
    parent: string | null,
    state: SubagentInfo['state'],
    waitingOn: SubagentInfo['waiting_on'],
    lastMessage: string | null,
    usage: Usage,
    created: number,
    mru: number,
    subagents: SubagentInfo[],
    tasks: Task[],
    archived = false
  ): SessionState {
    return {
      meta: demoMeta(id, title, created),
      entries: [],
      live: [],
      usage,
      turn: state === 'running' ? 'running' : 'idle',
      pending: [],
      parent,
      state,
      waiting_on: state === 'idle' ? waitingOn : null,
      archived,
      mru,
      subagents,
      tasks
    };
  }

  function demoChildren(): SessionState[] {
    const now = Date.now();
    return [
      demoChild('c1', 'provider hardening', 'a', 'demo', 'running', null, 'retry loop done, writing backoff tests', { input_tokens: 41200, output_tokens: 8900, total_tokens: 50100 }, now - 3600e3, now - 60e3, [], [demoTask('t1', 'in_progress', 'c1', 0)]),
      demoChild('c2', 'protocol surface', 'b', 'demo', 'idle', 'parent', 'types drafted — needs review sign-off', { input_tokens: 88000, output_tokens: 31000, total_tokens: 119000 }, now - 7200e3, now - 180e3, [
        demoSub('g1', 'cg1', 'fresh', 'idle', 'subagent', 'waiting on the field renames'),
        demoSub('g2', 'cg2', 'compacted', 'running', null, 'renaming the event groups')
      ], [demoTask('t4', 'blocked', 'c2', 1)]),
      demoChild('c3', 'config loader', 'c', 'demo', 'done', null, 'loader passes all tests', { input_tokens: 12400, output_tokens: 6100, total_tokens: 18500 }, now - 14400e3, now - 3600e3, [], [demoTask('t2', 'done', 'c3', 0)]),
      demoChild('c4', 'fixture generator', 'd', 'demo', 'failed', null, 'provider rejected the degenerate observation run', { input_tokens: 30100, output_tokens: 9400, total_tokens: 39500 }, now - 21600e3, now - 5400e3, [], [demoTask('t5', 'done', 'c4', 0)]),
      demoChild('c5', 'doc sweep', 'e', 'demo', 'stopped', null, 'stopped by user mid-sweep', { input_tokens: 5200, output_tokens: 2100, total_tokens: 7300 }, now - 28800e3, now - 7200e3, [], [])
    ];
  }

  function demoSub(handle: string, child: string, mode: SubagentInfo['context_mode'], state: SubagentInfo['state'], waitingOn: SubagentInfo['waiting_on'], lastMessage: string | null): SubagentInfo {
    return {
      handle,
      child,
      agent_type: 'general',
      context_mode: mode,
      state,
      waiting_on: waitingOn,
      last_message: lastMessage,
      usage: { input_tokens: 1200, output_tokens: 400, total_tokens: 1600 },
      task: null,
      resume_contract: null
    };
  }

  function demoSubagents(): SubagentInfo[] {
    return [
      demoSub('a', 'c1', 'compacted', 'running', null, 'writing backoff tests'),
      demoSub('b', 'c2', 'fork', 'idle', 'parent', 'types drafted — needs review sign-off'),
      demoSub('c', 'c3', 'fresh', 'done', null, 'loader passes all tests'),
      demoSub('d', 'c4', 'compacted', 'failed', null, 'provider rejected the degenerate run'),
      demoSub('e', 'c5', 'fresh', 'stopped', null, 'stopped by user mid-sweep')
    ];
  }

  function demoTask(id: string, status: Task['status'], worker: string, skip: number): Task {
    const now = Date.now();
    const base: Task = {
      id,
      title: { t1: 'Harden provider retry/backoff', t2: 'Config loader with layering', t3: 'Migrate tests to the 62-char alphabet', t4: 'Protocol surface: types & events', t5: '10k-entry fixture generator' }[id] ?? id,
      status,
      steps: [],
      criteria: [],
      evidence: [],
      blockers: [],
      notes: [],
      worker: { session: worker, status },
      created_in: 'demo',
      updated: now - 60e3 * (skip + 1),
      resume_contract: undefined,
      decisions: []
    };
    return base;
  }

  function demoTasks(): Task[] {
    const now = Date.now();
    const t1 = demoTask('t1', 'in_progress', 'c1', 0);
    t1.steps = [
      { text: 'inventory the retry paths', status: 'done', expected_output: 'retry-path list in the session' },
      { text: 'add backoff + jitter tests', status: 'active', expected_output: 'hardened retry/backoff tests' },
      { text: 'run the live acceptance', status: 'pending', expected_output: 'green live run log' }
    ];
    t1.criteria = [
      { text: 'retries on mid-stream provider errors', status: 'satisfied' },
      { text: 'backoff is capped', status: 'satisfied' },
      { text: 'live run passes', status: 'pending' }
    ];
    t1.evidence = [
      { criterion: 'retries on mid-stream provider errors', summary: 'mid-stream error preserves partial output', command: 'cargo test -p tau-core sse', passed: true },
      { criterion: 'backoff is capped', summary: 'backoff capped at 64 s', command: 'cargo test -p tau-core backoff', passed: true }
    ];
    t1.resume_contract = {
      task: 't1',
      title: t1.title,
      status: 'in_progress',
      current_step: { text: t1.steps[1].text, expected_output: t1.steps[1].expected_output },
      steps: t1.steps,
      evidence: t1.evidence,
      gaps: ['timeout path untested'],
      blockers: [],
      next_action: 'finish the backoff test, then the live acceptance'
    };
    const t2 = demoTask('t2', 'done', 'c3', 1);
    t2.steps = [
      { text: 'parse the TOML layers', status: 'done', expected_output: 'layering rules' },
      { text: 'reject unknown keys', status: 'done', expected_output: 'unknown-key test' }
    ];
    t2.criteria = [
      { text: 'project layer wins on collision', status: 'satisfied' },
      { text: 'unknown keys rejected', status: 'satisfied' }
    ];
    t2.evidence = [{ criterion: 'project layer wins on collision', summary: 'collision + unknown-key tests green', command: 'cargo test -p tau-core config', passed: true }];
    t2.decisions = [{ question: 'wholesale or field-level layering?', decision: 'field-level fallback per entry', decided_by: 'user' }];
    const t3 = demoTask('t3', 'pending', 'c1', 2);
    t3.steps = [
      { text: 'map base36 call sites', status: 'pending', expected_output: 'call-site list' },
      { text: 'port to the 62-char alphabet', status: 'pending', expected_output: 'ported + retested' }
    ];
    t3.criteria = [{ text: 'all tests green on the new alphabet', status: 'pending' }];
    t3.worker = undefined;
    const t4 = demoTask('t4', 'blocked', 'c2', 3);
    t4.steps = [
      { text: 'mirror the Rust types', status: 'done', expected_output: 'protocol.ts surface' },
      { text: 'wire the event groups', status: 'active', expected_output: 'subagent + task event cases' }
    ];
    t4.criteria = [
      { text: 'type-mirrors the crate field-for-field', status: 'satisfied' },
      { text: 'review sign-off', status: 'pending' }
    ];
    t4.evidence = [{ criterion: 'type-mirrors the crate field-for-field', summary: 'svelte-check green against the crate', passed: true }];
    t4.blockers = [{ reason: 'protocol types rejected in review', needs: 'resubmit after the N1 note' }];
    t4.resume_contract = {
      task: 't4',
      title: t4.title,
      status: 'blocked',
      current_step: { text: t4.steps[1].text, expected_output: t4.steps[1].expected_output },
      steps: t4.steps,
      evidence: t4.evidence,
      gaps: ['SubagentInfo shape pending review'],
      blockers: t4.blockers,
      next_action: 'address the review note, resubmit the types'
    };
    const t5 = demoTask('t5', 'done', 'c4', 4);
    t5.steps = [{ text: 'generate the 10k-entry fixture', status: 'done', expected_output: 'the committed fixture' }];
    t5.criteria = [{ text: 'snapshot stays under 2 MB', status: 'satisfied' }];
    t5.evidence = [{ criterion: 'snapshot stays under 2 MB', summary: 'measured 1.9 MB at 10k entries', command: 'cargo test -p tau-protocol snapshot', passed: true }];
    return [t1, t2, t3, t4, t5];
  }

  // Two live child streams: generated at 47 ms, flushed (coalesced) at
  // 25 ms — the prototype's exact cadence, through the real consumer.
  function startDemoStreams(): void {
    if (demoStreamsStarted) return;
    demoStreamsStarted = true;
    const ws = 'w-demo';
    const sid = 'demo';
    const lines = [
      '  let buf = &mut self.buf;',
      '  while let Some((i, line)) = scan_line(buf) {',
      '      match self.state {',
      '          St::Sse => self.on_line(line)?,',
      '          St::Json => self.on_chunk(line)?,',
      '      }',
      '  }',
      '  Ok(())'
    ];
    const streams = [
      { call: 'demo-live-1', tok: 0, buf: '', sent: 0, started: false },
      { call: 'demo-live-2', tok: 3, buf: '', sent: 0, started: false }
    ];
    const dirty = new Set<string>();
    setInterval(() => {
      for (const s of streams) {
        s.buf += (s.buf ? '\n' : '') + lines[s.tok % lines.length];
        s.tok++;
        dirty.add(s.call);
      }
    }, 47);
    setInterval(() => {
      if (dirty.size === 0) return;
      const batch: Event[] = [];
      for (const c of dirty) {
        const s = streams.find((x) => x.call === c)!;
        if (!s.started) {
          s.started = true;
          batch.push({ type: 'stream_start', workspace: ws, session: sid, call_id: s.call });
        }
        // Deltas are incremental (appended since the last flush, spec §8);
        // the consumer accumulates.
        const inc = s.buf.slice(s.sent);
        s.sent = s.buf.length;
        if (inc) {
          batch.push({
            type: 'stream_delta',
            workspace: ws,
            session: sid,
            call_id: s.call,
            text: inc,
            reasoning: null
          });
        }
      }
      dirty.clear();
      applyEvents(batch);
    }, 25);
  }

  function snapshotToState(snap: Snapshot): SessionState {
    const meta = snap.session;
    return {
      meta,
      // metadata skeleton: the card shows the preview until a paged read
      // replaces it with the payload.
      entries: snap.entries.map((m) => ({
        id: m.id,
        kind: m.kind === 'assistant' && m.status === 'interrupted' ? 'interrupted' : m.kind,
        text: m.preview
      })),
      live: [],
      usage: meta.usage,
      turn: snap.live.turn === 'running' ? 'running' : 'idle',
      pending: snap.live.queue.map((q) => ({ text: q.text, lane: laneOf(q.lane) })),
      parent: null,
      state: snap.live.turn === 'running' ? 'running' : 'idle',
      waiting_on: null,
      archived: false,
      mru: meta.created,
      subagents: snap.live.subagents,
      tasks: snap.live.tasks
    };
  }


  export async function openWorkspace(ws: Workspace): Promise<void> {
    if (store.demo) return;
    if (!store.workspaces.some((w) => w.id === ws.id)) store.workspaces.push(ws);
    const opened = await command({ type: 'workspace_open', cwd: ws.cwd });
    if (opened.kind === 'workspace') {
      const i = store.workspaces.findIndex((w) => w.id === ws.id);
      if (i >= 0) store.workspaces[i] = opened.workspace;
    }
    const list = await command({ type: 'session_list', workspace: ws.id });
    const sid =
      list.kind === 'sessions' && list.sessions.length > 0
        ? list.sessions[0].id
        : ((await command({
            type: 'session_new',
            workspace: ws.id,
            title: null
          })) as { kind: 'session'; session: SessionMeta }).session.id;
    await switchSession(sid);
  }

  // The v0 protocol has no "open folder" command (a workspace is a project
  // directory the core opens); the + menu's items are the demo's stand-in.
  export async function addWorkspace(): Promise<void> {
    if (!store.demo) return;
    const n = store.workspaces.length + 1;
    const ws: Workspace = { id: `w-demo-${n}`, name: `demo ${n}`, cwd: `~/git/tau${n}` };
    store.workspaces.push(ws);
    const { meta, entries, views } = buildDemoSession();
    const m2 = { ...meta, id: ws.id, workspace: ws.id, title: `Demo session ${n}` };
    demoViews = views;
    store.current = m2.id;
    store.sessions[m2.id] = {
      meta: m2,
      entries,
      live: [],
      usage: m2.usage,
      turn: 'idle',
      pending: [],
      parent: null,
      state: 'idle',
      waiting_on: null,
      archived: false,
      mru: m2.created,
      subagents: [],
      tasks: []
    };
  }

  export async function closeWorkspace(ws: Workspace): Promise<void> {
    if (!store.demo) {
      const list = await command({ type: 'session_list', workspace: ws.id }).catch(() => ({
        kind: 'sessions' as const,
        sessions: []
      }));
      if (list.kind === 'sessions') {
        for (const s of list.sessions) {
          await command({ type: 'session_close', session: s.id }).catch(() => {});
        }
      }
    }
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
      next.mru = prev.mru;
    }
    store.sessions[sid] = next;
  }

  // A pane action (session tree row / sub-agent double-click): the child is
  // an ordinary session — opening it switches the current view. The demo
  // serves it from the prebuilt dataset; the live path fetches the snapshot.
  export async function openSessionById(sid: string): Promise<void> {
    if (store.demo) {
      if (store.sessions[sid]) store.current = sid;
      return;
    }
    await switchSession(sid);
  }

  // Paged read around the viewport (spec §8): the GUI decides the window,
  // the core serves the slice. The demo answers from its in-memory views.
  export async function fetchWindow(sid: string, start: number, count: number): Promise<void> {
    const s = store.sessions[sid];
    if (!s || count <= 0) return;
    let views: ViewEntry[];
    if (store.demo) {
      views = demoViews.slice(start, start + count);
    } else {
      const out = await command({
        type: 'session_entries',
        session: sid,
        since: null,
        range: { start, count }
      });
      if (out.kind !== 'entries') return;
      views = out.entries;
    }
    for (const v of views) {
      const i = s.entries.findIndex((e) => e.id === v.id);
      if (i < 0) continue;
      const next = toEntry(v);
      const old = s.entries[i];
      if (old.text !== next.text || old.output !== next.output || old.status !== next.status) {
        s.entries[i] = next;
      }
    }
  }

  export async function send(text: string, lane: PendingMsg['lane']): Promise<void> {
    const sid = store.current;
    if (sid === null || text.trim() === '') return;
    const s = sessionOf(sid);
    s.pending = s.pending.filter((p) => !(p.text === text && p.lane === lane));
    if (store.demo) {
      // The demo answers with one canned streamed turn through the same
      // event consumer the live path uses.
      const n = demoViews.length;
      const id = String(n).padStart(8, '0');
      const userView: ViewEntry = {
        id,
        parent: s.meta.leaf,
        kind: 'user',
        timestamp: Date.now(),
        payload: { text, lane: lane === 'follow-up' ? 'follow_up' : lane },
        blob: null,
        first_kept: null
      };
      demoViews.push(userView);
      s.entries.push(toEntry(userView));
      s.meta.leaf = id;
      const callId = 'demo-reply-' + id;
      const answer = 'On it. ' + text + '\n```rust\nlet n = 0;\n```\nDone — **verified**.';
      const usage: Usage = { input_tokens: 1200, output_tokens: 40, total_tokens: 1240 };
      applyEvents([
        { type: 'stream_start', workspace: s.meta.workspace, session: sid, call_id: callId },
        { type: 'stream_delta', workspace: s.meta.workspace, session: sid, call_id: callId, text: answer, reasoning: null },
        {
          type: 'stream_end',
          workspace: s.meta.workspace,
          session: sid,
          call_id: callId,
          interrupted: false,
          usage
        }
      ]);
      return;
    }
    try {
      await command({
        type: 'message_send',
        session: sid,
        text,
        lane: lane === 'follow-up' ? 'follow_up' : lane
      });
    } catch (e) {
      store.error = e instanceof Error ? e.message : String(e);
    }
  }

  export async function stop(): Promise<void> {
    const sid = store.current;
    if (sid === null) return;
    if (store.demo) return;
    try {
      await command({ type: 'message_stop', session: sid });
    } catch (e) {
      store.error = e instanceof Error ? e.message : String(e);
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
    if (store.demo) return;
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
      const sid = ev.session;
      if (!sid) {
        if (ev.type === 'system') {
          store.error = ev.kind.kind === 'error' ? ev.kind.message : null;
        }
        continue;
      }
      const s = store.sessions[sid];
      if (!s) continue;
      switch (ev.type) {
        case 'stream_start': {
          const le = { id: ev.call_id, text: '', reasoning: '' };
          const pd = pendingDeltas.get(ev.call_id);
          if (pd) {
            le.text = pd.text;
            le.reasoning = pd.reasoning;
            pendingDeltas.delete(ev.call_id);
          }
          s.live.push(le);
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
              le = { id: ev.call_id, text: pd.text, reasoning: pd.reasoning };
            }
          }
          if (le) {
            s.entries.push({
              id: le.id,
              kind: ev.interrupted ? 'interrupted' : 'message',
              text: le.text,
              reasoning: le.reasoning || undefined,
              usage: ev.usage ?? undefined
            });
          }
          if (ev.usage) s.usage = ev.usage;
          if (s.live.length === 0) s.turn = 'idle';
          s.meta.leaf = le?.id ?? s.meta.leaf;
          break;
        }
        case 'tool_start': {
          const existing = s.entries.find((e) => e.id === ev.call_id);
          if (existing) {
            existing.status = 'running';
          } else {
            s.entries.push({ id: ev.call_id, kind: 'tool', name: ev.name, status: 'running' });
          }
          break;
        }
        case 'tool_end': {
          const e = s.entries.find((x) => x.id === ev.call_id);
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
        case 'session_event': {
          if (ev.kind.kind === 'branch_move') s.meta.leaf = ev.kind.leaf;
          break;
        }
        case 'subagent': {
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
            s.subagents = s.subagents.filter((x) => x.handle !== k.handle);
            s.subagents.push(info);
            touchChild(ev.session, k.child, 'running', now);
            break;
          }
          if (k.kind === 'state') {
            const st = k.state as 'running' | 'idle' | 'done' | 'failed' | 'stopped';
            const info = s.subagents.find((x) => x.handle === k.handle);
            if (info) {
              info.state = st;
              info.waiting_on = typeof k.detail === 'string' ? k.detail : info.waiting_on;
              if (k.note) info.last_message = k.note;
            }
            touchChild(ev.session, k.child, st, now);
            break;
          }
          // notified: a child notification reached the parent.
          const child = store.sessions[k.child];
          if (child) child.mru = now;
          const info = s.subagents.find((x) => x.child === k.child);
          if (info) {
            info.last_message = k.text;
            info.state = k.wake === 'done' ? 'done' : k.wake === 'failed' ? 'failed' : 'idle';
            if (info.state === 'idle') info.waiting_on = 'parent';
          }
          if (child) {
            child.state = k.wake === 'done' ? 'done' : k.wake === 'failed' ? 'failed' : 'idle';
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
