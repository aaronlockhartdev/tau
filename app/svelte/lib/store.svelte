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
  }

  export const store = $state({
    focus: false,
    workspaces: [] as Workspace[],
    current: null as string | null,
    sessions: {} as Record<string, SessionState>,
    loading: false,
    error: null as string | null,
    demo: false
  });

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
      ]
    };
    startDemoStreams();
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
      pending: snap.live.queue.map((q) => ({ text: q.text, lane: laneOf(q.lane) }))
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
      pending: []
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
    const cur = store.current;
    if (cur && sessionOf(cur).meta.workspace === ws.id) {
      store.current = null;
    }
  }

  export async function switchSession(sid: string): Promise<void> {
    store.current = sid;
    const out = await command({ type: 'session_open', session: sid });
    if (out.kind !== 'snapshot') throw new Error('unexpected session_open output');
    store.sessions[sid] = snapshotToState(out.snapshot);
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

  export async function deleteQueueItem(text: string, lane: PendingMsg['lane']): Promise<void> {
    const sid = store.current;
    if (sid === null) return;
    const s = sessionOf(sid);
    s.pending = s.pending.filter((p) => !(p.text === text && p.lane === lane));
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
          s.live.push({ id: ev.call_id, text: '', reasoning: '' });
          s.turn = 'running';
          break;
        }
        case 'stream_delta': {
          const le = s.live.find((x) => x.id === ev.call_id);
          if (!le) break;
          le.text += ev.text;
          if (ev.reasoning) le.reasoning += ev.reasoning;
          break;
        }
        case 'stream_end': {
          const le = s.live.find((x) => x.id === ev.call_id);
          s.live = s.live.filter((x) => x.id !== ev.call_id);
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
        case 'system': {
          store.error = ev.kind.kind === 'error' ? ev.kind.message : null;
          break;
        }
      }
    }
  }
</script>
