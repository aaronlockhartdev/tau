// @vitest-environment jsdom
// Store-level suite (roadmap 1c): boot, applyEvents (streaming, tools,
// queueing, sub-agent mirror), session switch + archive convergence, and
// the refetch-wave guards — the browser rig's checks, at node speed.
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => mockInvoke(...args) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => () => {}) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(() => Promise.resolve(null)) }));
import { type Command, type SkillInfo, type Workspace } from './protocol';
import { applySessionList, openSession, touchChild } from './sessions';
import {
  applyEvents,
  closeWorkspace,
  closeWorkspaces,
  init,
  openWorkspace,
  retryDirFetch,
  send,
  store,
  switchSession,
  tabSelect,
  toggleFileDir
} from './store.svelte';
import { USAGE, WS, bindIPC, entryView, freshStore, meta, snap, sub } from './testing/store-harness';

const { mockIPC, defaultIPC } = bindIPC(mockInvoke);
beforeEach(() => {
  freshStore();
  mockInvoke.mockReset();
  // A safe no-op default; tests that expect a specific route override it.
  mockInvoke.mockImplementation(() => Promise.resolve({ kind: 'none' as const }));
  vi.useRealTimers();
});

describe('boot (init)', () => {
  it('opens the first listed session from its snapshot', async () => {
    defaultIPC();
    await init();
    expect(store.workspaces).toEqual([WS]);
    expect(store.current).toBe('s1');
    const s = store.sessions['s1']!;
    expect(s).toBeTruthy();
    expect(s.meta.id).toBe('s1');
    expect(store.files[WS.id]).toEqual({});
  });

  it('creates a session when the workspace is empty', async () => {
    defaultIPC({ session_list: () => ({ kind: 'sessions', sessions: [] }) });
    await init();
    expect(store.current).toBe('s2');
    expect(store.sessions['s2']).toBeTruthy();
    const newCall = mockInvoke.mock.calls.find((c) => (c[1] as { command: Command }).command.type === 'session_new');
    expect(newCall).toBeTruthy();
  });

  it('opens the most recent unarchived session, skipping archived ones', async () => {
    // list_workspace emits sessions/ then archive/ in arbitrary order, so the
    // first listed can be archived; the auto-open must pick the newest live one.
    defaultIPC({
      session_list: () => ({
        kind: 'sessions',
        sessions: [
          meta('archived-old', WS.id, { archived: true, created: 500 }),
          meta('live-new', WS.id, { created: 900 }),
          meta('live-old', WS.id, { created: 700 })
        ]
      })
    });
    await init();
    expect(store.current).toBe('live-new');
  });

  it('fails fast outside the Tauri window', async () => {
    delete (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
    await init();
    expect(store.error).toBe('no Tauri window — run the app');
    expect(store.loading).toBe(false);
    expect(mockInvoke).not.toHaveBeenCalled();
  });
});

describe('openWorkspace concurrency', () => {
  const WS2: Workspace = { id: 'w2', name: 'other', cwd: '/tmp/other' };

  it('a different-tab click while an open is in flight proceeds after it; a same-tab double-click is one open', async () => {
    let release: () => void;
    const gate = new Promise<void>((r) => {
      release = r;
    });
    const opened: string[] = [];
    mockIPC((cmd) => {
      if (cmd.type === 'workspace_open') {
        if (cmd.cwd === WS.cwd) {
          opened.push('w1');
          return gate.then(() => ({ kind: 'workspace', workspace: WS }));
        }
        opened.push('w2');
        return { kind: 'workspace', workspace: WS2 };
      }
      if (cmd.type === 'skill_list') return { kind: 'skills', skills: [] };
      if (cmd.type === 'session_list')
        return cmd.workspace === WS.id
          ? { kind: 'sessions', sessions: [meta('s1')] }
          : { kind: 'sessions', sessions: [meta('s3', WS2.id)] };
      if (cmd.type === 'session_open')
        return cmd.session === 's1'
          ? snap('s1')
          : { kind: 'snapshot', snapshot: { ...snap('s3').snapshot, workspace: WS2, session: meta('s3', WS2.id) } };
      return { kind: 'none' };
    });
    const p1 = openWorkspace(WS);
    const p2 = openWorkspace(WS);
    const p3 = openWorkspace(WS2);
    release!();
    await Promise.all([p1, p2, p3]);
    expect(opened).toEqual(['w1', 'w2']);
    expect(store.workspaces.map((w) => w.id).sort()).toEqual(['w1', 'w2']);
    expect(store.current).toBe('s3');
  });
});

describe('applyEvents: streaming (entry_upsert, ADR-0008)', () => {
  function oneSession() {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    return store.sessions['s1']!;
  }
  function upsert(id: string, kind: string, payload: Record<string, unknown>) {
    return {
      type: 'entry_upsert' as const,
      workspace: WS.id,
      session: 's1',
      entry: entryView(id, kind, payload)
    };
  }
  // #303: one frame's growth of a streaming assistant entry.
  function delta(id: string, seq: number, text: string, reasoning = '') {
    return {
      type: 'stream_delta' as const,
      workspace: WS.id,
      session: 's1',
      id,
      seq,
      text,
      reasoning
    };
  }

  // The wire shape the app's production tee sends (harness/launch.rs), read
  // as the protocol declares it: every frame of a run grows the card (#303).
  it('grows a card from every frame of the wire event the app sends', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' },
      { type: 'stream_delta', workspace: WS.id, session: 's1', id: '00000001', seq: 1, text: 'li', reasoning: '' },
      { type: 'stream_delta', workspace: WS.id, session: 's1', id: '00000001', seq: 2, text: 've', reasoning: '' }
    ]);
    const card = s.entries['00000001']!;
    expect(card.kind).toBe('message');
    if (card.kind === 'message') expect(card.text).toBe('live');
  });

  it('a turn that fails before its first output leaves "starting" (no stuck running badge)', () => {
    const s = oneSession();
    s.turn = 'starting';
    applyEvents([
      { type: 'system', workspace: WS.id, session: 's1', kind: { kind: 'error', message: 'provider 4xx' } }
    ]);
    expect(s.turn).toBe('idle');
  });

  it('streams a turn to completion: upserts grow the card, stream_end idles the turn with usage', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' },
      upsert('00000001', 'assistant', { text: 'hel', reasoning: 'thinking', interrupted: false }),
      upsert('00000001', 'assistant', { text: 'hello', reasoning: 'thinking', interrupted: false }),
      { type: 'stream_end', workspace: WS.id, session: 's1', call_id: 'c1', interrupted: false, usage: USAGE }
    ]);
    expect(s.turn).toBe('idle');
    expect(s.usage).toEqual(USAGE);
    const e = s.entries['00000001']!;
    expect(e).toBeTruthy();
    expect(e.kind).toBe('message');
    if (e.kind === 'message' || e.kind === 'interrupted') {
      expect(e.text).toBe('hello');
      expect(e.reasoning).toBe('thinking');
    }
  });

  it('a card renders on first sight; a re-emission updates it in place, never repositioning', () => {
    const s = oneSession();
    applyEvents([
      upsert('00000001', 'user', { text: 'hi' }),
      upsert('00000002', 'assistant', { text: 'a', interrupted: false }),
      upsert('00000001', 'user', { text: 'hi' })
    ]);
    expect(Object.keys(s.entries)).toEqual(['00000001', '00000002']);
    expect(s.entries['00000001']?.kind).toBe('user');
  });

  it('marks an interrupted response as such (the payload flag decides the kind)', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' },
      upsert('00000001', 'assistant', { text: 'ab', interrupted: true }),
      { type: 'stream_end', workspace: WS.id, session: 's1', call_id: 'c1', interrupted: true, usage: null }
    ]);
    expect(s.entries['00000001']?.kind).toBe('interrupted');
  });

  it('prefers the server duration over the wall clock for TPS', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
    const s = oneSession();
    applyEvents([{ type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' }]);
    await vi.advanceTimersByTimeAsync(2000);
    applyEvents([
      { type: 'stream_end', workspace: WS.id, session: 's1', call_id: 'c1', interrupted: false, usage: USAGE, duration_secs: 1 }
    ]);
    // 10 output tokens over the server's 1 s — not 5 over the 2 s wall clock.
    expect(s.tps).toBeCloseTo(10);
    vi.useRealTimers();
  });

  it('measures wall-clock TPS from the first delta, excluding prefill', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
    const s = oneSession();
    applyEvents([{ type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' }]);
    // A 2 s prefill gap before the first token: it must not count.
    await vi.advanceTimersByTimeAsync(2000);
    applyEvents([upsert('00000001', 'assistant', { text: 'a', interrupted: false })]);
    await vi.advanceTimersByTimeAsync(1000);
    applyEvents([
      { type: 'stream_end', workspace: WS.id, session: 's1', call_id: 'c1', interrupted: false, usage: USAGE }
    ]);
    // 10 tokens over the 1 s after the first delta — not 3.33 over the 3 s
    // wall clock that a stream-start base would give.
    expect(s.tps).toBeCloseTo(10);
    vi.useRealTimers();
  });

  it('reports no TPS when the stream ends before any token', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
    const s = oneSession();
    applyEvents([{ type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' }]);
    await vi.advanceTimersByTimeAsync(1000);
    // Usage arrived but no assistant delta ever did: no first-delta clock,
    // so no TPS (and no divide by zero).
    applyEvents([
      { type: 'stream_end', workspace: WS.id, session: 's1', call_id: 'c1', interrupted: false, usage: USAGE }
    ]);
    expect(s.tps).toBe(0);
    vi.useRealTimers();
  });

  it('runs the live TPS estimate from the first delta', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
    const s = oneSession();
    applyEvents([{ type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' }]);
    // 2 s prefill, then the first character opens the estimate's clock.
    await vi.advanceTimersByTimeAsync(2000);
    applyEvents([delta('00000001', 1, 'a')]);
    await vi.advanceTimersByTimeAsync(2000);
    // ~2 s after the first delta, 80 chars accumulated ≈ 20 tokens → 10 tps.
    applyEvents([delta('00000001', 2, 'x'.repeat(79))]);
    expect(s.tps).toBeCloseTo(10);
    vi.useRealTimers();
  });

  it('the deltas of a stream grow one card in seq order', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' },
      delta('00000001', 1, 'hel', 'think'),
      delta('00000001', 2, 'lo'),
      delta('00000001', 3, ' world')
    ]);
    const e = s.entries['00000001']!;
    expect(e.kind).toBe('message');
    if (e.kind === 'message') {
      expect(e.text).toBe('hello world');
      expect(e.reasoning).toBe('think');
    }
  });

  it('the committed entry replaces the card the deltas built, and carries the usage', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' },
      delta('00000001', 1, 'hel'),
      delta('00000001', 2, 'lo'),
      upsert('00000001', 'assistant', { text: 'hello world', interrupted: false, usage: USAGE }),
      { type: 'stream_end', workspace: WS.id, session: 's1', call_id: 'c1', interrupted: false, usage: USAGE, duration_secs: 1 }
    ]);
    const e = s.entries['00000001']!;
    if (e.kind === 'message') {
      expect(e.text).toBe('hello world');
      expect(e.usage).toEqual(USAGE);
    }
  });

  it('a delta out of order is refused, and the file line recovers the text', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' },
      delta('00000001', 1, 'one '),
      delta('00000001', 3, 'three ')
    ]);
    const partial = s.entries['00000001']!;
    if (partial.kind === 'message') expect(partial.text).toBe('one ');
    // The dropped frame is not guessed at; the entry's line is the recovery.
    applyEvents([upsert('00000001', 'assistant', { text: 'one two three', interrupted: false })]);
    const e = s.entries['00000001']!;
    if (e.kind === 'message') expect(e.text).toBe('one two three');
  });

  it('a delta for an unknown session changes nothing', () => {
    oneSession();
    applyEvents([{ ...delta('00000001', 1, 'orphan'), session: 's9' }]);
    expect(store.sessions['s9']).toBeUndefined();
  });

  it('a tool call is one card: the call upsert starts it, the result upsert fills the output', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' },
      upsert('00000001', 'assistant', { text: 'run ls', interrupted: false }),
      upsert('00000002', 'tool', { call_id: 't1', name: 'bash', args: { command: 'ls' } }),
      upsert('00000002', 'tool', { call_id: 't1', name: 'bash', args: { command: 'ls' }, output: 'exit 0\n--- stdout ---\nok' }),
      { type: 'stream_end', workspace: WS.id, session: 's1', call_id: 'c1', interrupted: false, usage: null }
    ]);
    const tools = Object.values(s.entries).filter((e) => e.kind === 'tool');
    expect(tools).toHaveLength(1);
    const tool = tools[0]!;
    if (tool.kind === 'tool') {
      expect(tool.name).toBe('bash');
      expect(tool.status).toBe('ok');
      expect(tool.output).toBe('exit 0\n--- stdout ---\nok');
    }
  });

  it('keeps two tools of one call as two cards (one id each)', () => {
    const s = oneSession();
    applyEvents([
      upsert('00000001', 'tool', { call_id: 't1', name: 'a', output: 1 }),
      upsert('00000002', 'tool', { call_id: 't2', name: 'b', output: 2 })
    ]);
    const tools = Object.values(s.entries).filter((e) => e.kind === 'tool');
    expect(tools).toHaveLength(2);
  });
});

describe('applyEvents: queueing', () => {
  it('maps the wire lanes onto the composer lanes', () => {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    applyEvents([
      {
        type: 'queue',
        workspace: WS.id,
        session: 's1',
        items: [
          { text: 'a', lane: 'force' },
          { text: 'b', lane: 'steering' },
          { text: 'c', lane: 'follow_up' }
        ]
      }
    ]);
    expect(store.sessions['s1']!.pending.map((p) => p.lane)).toEqual(['force', 'steering', 'follow-up']);
  });

  it('a sourced item (a sub-agent report) keeps its provenance', () => {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    applyEvents([
      {
        type: 'queue',
        workspace: WS.id,
        session: 's1',
        items: [
          { text: 'a', lane: 'steering' },
          { text: 'child done', lane: 'steering', source: 's1-1' }
        ]
      }
    ]);
    const p = store.sessions['s1']!.pending;
    expect(p).toHaveLength(2);
    expect(p[0]!.source).toBeUndefined();
    expect(p[1]!.source).toBe('s1-1');
  });

  it('send(): the user card lands on the core upsert echo (no optimistic card), the lane maps, the duplicate queues dedupe', async () => {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    store.current = 's1';
    applyEvents([
      { type: 'queue', workspace: WS.id, session: 's1', items: [{ text: 'hi', lane: 'steering' }] }
    ]);
    const before = store.tailJump;
    await send('hi', 'steering');
    expect(store.sessions['s1']!.pending).toHaveLength(0);
    // ADR-0008: no client-minted card — the core's echo is the card.
    expect(Object.keys(store.sessions['s1']!.entries)).toEqual([]);
    applyEvents([
      {
        type: 'entry_upsert',
        workspace: WS.id,
        session: 's1',
        entry: entryView('00000001', 'user', { text: 'hi', lane: 'steering' })
      }
    ]);
    const card = store.sessions['s1']!.entries['00000001']!;
    expect(card).toBeTruthy();
    if (card.kind === 'user') expect(card.text).toBe('hi');
    expect(store.tailJump).toBe(before + 1);
    const calls = mockInvoke.mock.calls.map((c) => (c[1] as { command: Command }).command);
    expect(calls.at(-1)).toEqual({ type: 'message_send', session: 's1', text: 'hi', lane: 'steering' });
  });

  it('a failed send leaves no card (no ghost, no twin on retry)', async () => {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    store.current = 's1';
    defaultIPC({
      message_send: () => {
        throw new Error('provider 500');
      }
    });
    await send('hi', 'steering');
    expect(store.error).toBe('provider 500');
    expect(Object.keys(store.sessions['s1']!.entries)).toEqual([]);
    // a retry of the same text must not stack a second ghost
    await send('hi', 'steering');
    expect(Object.keys(store.sessions['s1']!.entries)).toEqual([]);
  });
});

describe('applyEvents: last_modified (#282/#283)', () => {
  function freshSession(): void {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    store.sessions['s1']!.last_modified = 1000;
  }

  it('stream_start and stream_end advance the timestamp (mock clock at 5000)', () => {
    freshSession();
    vi.spyOn(Date, 'now').mockReturnValue(5000);
    applyEvents([{ type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' }]);
    expect(store.sessions['s1']!.last_modified).toBe(5000);
    applyEvents([
      {
        type: 'stream_end',
        workspace: WS.id,
        session: 's1',
        call_id: 'c1',
        interrupted: false,
        usage: null
      }
    ]);
    expect(store.sessions['s1']!.last_modified).toBe(5000);
    vi.restoreAllMocks();
  });

  it('a tool entry_upsert (the user send echo, ADR-0008) advances the timestamp', () => {
    freshSession();
    vi.spyOn(Date, 'now').mockReturnValue(5000);
    applyEvents([
      {
        type: 'entry_upsert',
        workspace: WS.id,
        session: 's1',
        entry: entryView('00000001', 'tool', { tool: 'bash', args: 'ls' })
      }
    ]);
    expect(store.sessions['s1']!.last_modified).toBe(5000);
    vi.restoreAllMocks();
  });

  it('queue and plan_changed advance the timestamp', () => {
    freshSession();
    vi.spyOn(Date, 'now').mockReturnValue(5000);
    applyEvents([{ type: 'queue', workspace: WS.id, session: 's1', items: [{ text: 'hi', lane: 'steering' }] }]);
    expect(store.sessions['s1']!.last_modified).toBe(5000);
    applyEvents([
      {
        type: 'plan_changed',
        workspace: WS.id,
        session: 's1',
        plan: { steps: [{ text: 'a', done: false }], criteria: '', notes: '' }
      }
    ]);
    expect(store.sessions['s1']!.last_modified).toBe(5000);
    vi.restoreAllMocks();
  });

  it('subagent activity advances the parent timestamp (spawn, then a notified wake)', () => {
    freshSession();
    vi.spyOn(Date, 'now').mockReturnValue(5000);
    applyEvents([
      {
        type: 'subagent_event',
        workspace: WS.id,
        session: 's1',
        kind: { kind: 'spawned', handle: 'h1', child: 'c1', agent_type: 'general', context_mode: 'fresh', title: 'worker' }
      }
    ]);
    expect(store.sessions['s1']!.last_modified).toBe(5000);
    applyEvents([
      {
        type: 'subagent_event',
        workspace: WS.id,
        session: 's1',
        kind: { kind: 'notified', child: 'c1', wake: 'done', text: 'result', output: null }
      }
    ]);
    expect(store.sessions['s1']!.last_modified).toBe(5000);
    vi.restoreAllMocks();
  });

  it('om_status, session_event, and system errors do not advance the timestamp', () => {
    freshSession();
    vi.spyOn(Date, 'now').mockReturnValue(5000);
    applyEvents([
      { type: 'om_status', workspace: WS.id, session: 's1', kind: 'observing' },
      { type: 'session_event', workspace: WS.id, session: 's1', kind: { kind: 'branch_move', leaf: 'abc' } },
      { type: 'system', workspace: WS.id, session: 's1', kind: { kind: 'error', message: 'boom' } }
    ]);
    expect(store.sessions['s1']!.last_modified).toBe(1000);
    vi.restoreAllMocks();
  });
});

describe('session switch', () => {
  it('replaces the stub with the snapshot and keeps the parent link', async () => {
    store.sessions = applySessionList({}, [meta('p'), meta('c', WS.id, { parent: 'p' })]);
    defaultIPC({ session_open: () => snap('c', { session: { title: 'child' } }) });
    await switchSession('c');
    const s = store.sessions['c']!;
    expect(store.current).toBe('c');
    expect(s.parent).toBe('p');
    expect(s.meta.title).toBe('child');
  });

  it('a failed session_open rolls current back to the previous session and surfaces the error', async () => {
    store.sessions = applySessionList({}, [meta('s1'), meta('s2')]);
    store.current = 's1';
    defaultIPC({
      session_open: (cmd) => {
        if (cmd.type === 'session_open' && cmd.session === 's2') throw new Error('session file missing');
        return snap('s1');
      }
    });
    await switchSession('s2');
    expect(store.error).toBe('session file missing');
    expect(store.current).toBe('s1');
  });

  it('re-opening a session preserves the row last_modified (no tree reshuffle away from the clicked row)', async () => {
    store.sessions = applySessionList({}, [meta('s1')]);
    store.sessions['s1']!.last_modified = 9999;
    defaultIPC();
    await switchSession('s1');
    expect(store.sessions['s1']!.last_modified).toBe(9999);
  });

  it('self-heals the child stubs from the snapshot (fills a lost spawn, corrects a stale one, keeps the last_modified)', async () => {
    store.sessions = applySessionList({}, [meta('p')]);
    store.sessions = touchChild(store.sessions, 'p', 'c', 'running', 5000, null);
    defaultIPC({
      session_open: () =>
        snap('p', {
          live: {
            subagents: [
              sub('c', { state: 'done', waiting_on: 'parent' }),
              sub('d', { state: 'running' })
            ]
          }
        })
    });
    await switchSession('p');
    expect(store.sessions['c']!.state).toBe('done');
    expect(store.sessions['c']!.last_modified).toBe(5000);
    expect(store.sessions['d']).toBeTruthy();
    expect(store.sessions['d']!.state).toBe('running');
    expect(store.sessions['d']!.parent).toBe('p');
  });

  it('synthesizes the tab entry for a disk-restored child with no live supervisor', async () => {
    store.sessions = touchChild(applySessionList({}, [meta('p')]), 'p', 'c', 'done', 1, 'parent');
    defaultIPC({ session_open: () => snap('p') });
    await switchSession('p');
    expect(store.sessions['p']!.subagents).toEqual([sub('c', { state: 'done' })]);
  });

  it('a branch_move event moves the leaf', () => {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    applyEvents([
      { type: 'session_event', workspace: WS.id, session: 's1', kind: { kind: 'branch_move', leaf: '7' } }
    ]);
    expect(store.sessions['s1']!.meta.leaf).toBe('7');
  });
});

describe('closeWorkspace', () => {
  const WS2: Workspace = { id: 'w2', name: 'other', cwd: '/tmp/other' };

  it('closing the current workspace lands on a surviving session; the last close clears current', async () => {
    store.workspaces = [WS, WS2];
    // The survivor's sessions are NOT in the map: the redirect must re-open
    // the surviving workspace (workspace_open + session_list + session_open),
    // not just pick from already-hydrated sessions (dogfood B1).
    store.sessions = applySessionList({}, [meta('s1')]);
    store.current = 's1';
    mockIPC((c) => {
      switch (c.type) {
        case 'workspace_open':
          return { kind: 'workspace', workspace: WS2 };
        case 'skill_list':
          return { kind: 'skills', skills: [] };
        case 'session_list':
          return { kind: 'sessions', sessions: [meta('s2', WS2.id)] };
        case 'file_list':
          return { kind: 'files', files: [] };
        case 'session_open':
          return snap('s2', { session: { workspace: WS2.id } });
        default:
          return { kind: 'none' };
      }
    });
    await closeWorkspace(WS);
    expect(store.workspaces).toEqual([WS2]);
    expect(store.current).toBe('s2');
    await closeWorkspace(WS2);
    expect(store.workspaces).toEqual([]);
    expect(store.current).toBeNull();
    expect(store.sessions['s1']).toBeUndefined();
    expect(store.sessions['s2']).toBeUndefined();
  });

  it('closing a non-current workspace leaves current alone', async () => {
    store.workspaces = [WS, WS2];
    store.sessions = applySessionList({}, [meta('s1'), meta('s2', WS2.id)]);
    store.current = 's2';
    mockIPC(() => ({ kind: 'none' }));
    await closeWorkspace(WS);
    expect(store.current).toBe('s2');
  });

  it('sends the workspace_close command to persist the close', async () => {
    store.workspaces = [WS];
    store.sessions = applySessionList({}, [meta('s1')]);
    store.current = null;
    const calls: string[] = [];
    mockIPC((cmd) => {
      calls.push(cmd.type);
      return { kind: 'none' };
    });
    await closeWorkspace(WS);
    expect(calls).toContain('workspace_close');
    expect(calls.at(-1)).toBe('workspace_close');
  });

  it('a failed persist surfaces store.error; the tab still closes locally', async () => {
    store.workspaces = [WS];
    store.sessions = applySessionList({}, [meta('s1')]);
    store.current = null;
    mockIPC((cmd) => {
      if (cmd.type === 'workspace_close') throw new Error('registry write failed');
      return { kind: 'none' };
    });
    await closeWorkspace(WS);
    expect(store.error).toBe('registry write failed');
    expect(store.workspaces).toEqual([]);
  });

  it('a closed tab drops out of the selection (no dangling member)', async () => {
    store.workspaces = [WS, WS2];
    store.sessions = applySessionList({}, [meta('s1'), meta('s2', WS2.id)]);
    store.current = 's2';
    store.tabSelected = ['w1', 'w2'];
    mockIPC(() => ({ kind: 'none' }));
    await closeWorkspace(WS);
    expect(store.tabSelected).toEqual(['w2']);
  });
});

describe('tab select (B2)', () => {
  const WS2: Workspace = { id: 'w2', name: 'other', cwd: '/tmp/other' };
  const WS3: Workspace = { id: 'w3', name: 'third', cwd: '/tmp/third' };
  const plain = { shiftKey: false, metaKey: false, ctrlKey: false };
  const cmd = { shiftKey: false, metaKey: true, ctrlKey: false };
  const shift = { shiftKey: true, metaKey: false, ctrlKey: false };

  it('cmd/ctrl toggles a tab in and out of the selection', () => {
    store.workspaces = [WS, WS2];
    tabSelect('w1', cmd);
    expect(store.tabSelected).toEqual(['w1']);
    expect(store.tabSelAnchor).toBe('w1');
    tabSelect('w2', { ...cmd, metaKey: false, ctrlKey: true });
    expect(store.tabSelected).toEqual(['w1', 'w2']);
    tabSelect('w1', cmd);
    expect(store.tabSelected).toEqual(['w2']);
  });

  it('shift spans a range in tab order, from the held anchor', () => {
    store.workspaces = [WS, WS2, WS3];
    tabSelect('w1', cmd);
    tabSelect('w3', shift);
    expect(store.tabSelected).toEqual(['w1', 'w2', 'w3']);
    tabSelect('w2', shift);
    expect(store.tabSelected).toEqual(['w1', 'w2']);
  });

  it('a plain click clears the selection and sets the anchor', () => {
    store.workspaces = [WS, WS2];
    store.tabSelected = ['w2'];
    store.tabSelAnchor = 'w2';
    tabSelect('w1', plain);
    expect(store.tabSelected).toEqual([]);
    expect(store.tabSelAnchor).toBe('w1');
  });
});

describe('closeWorkspaces (bulk, B2)', () => {
  const WS2: Workspace = { id: 'w2', name: 'other', cwd: '/tmp/other' };
  const WS3: Workspace = { id: 'w3', name: 'third', cwd: '/tmp/third' };

  it('closes N tabs; the current-redirect lands on the final survivor', async () => {
    store.workspaces = [WS, WS2, WS3];
    store.sessions = applySessionList({}, [meta('s1'), meta('s2', WS2.id), meta('s3', WS3.id)]);
    store.current = 's1';
    store.tabSelected = ['w1', 'w2'];
    store.tabSelAnchor = 'w1';
    mockIPC((c) => {
      switch (c.type) {
        case 'workspace_open':
          return { kind: 'workspace', workspace: WS3 };
        case 'session_list':
          return { kind: 'sessions', sessions: [meta('s3', WS3.id)] };
        case 'file_list':
          return { kind: 'files', files: [] };
        case 'session_open':
          return snap('s3', { session: { workspace: WS3.id } });
        default:
          return { kind: 'none' };
      }
    });
    await closeWorkspaces([WS, WS2]);
    expect(store.workspaces).toEqual([WS3]);
    expect(store.current).toBe('s3');
    expect(store.tabSelected).toEqual([]);
    expect(store.tabSelAnchor).toBeNull();
  });

  it('a one-element bulk is a single close', async () => {
    store.workspaces = [WS, WS2];
    store.sessions = applySessionList({}, [meta('s1'), meta('s2', WS2.id)]);
    store.current = 's2';
    mockIPC(() => ({ kind: 'none' }));
    await closeWorkspaces([WS]);
    expect(store.workspaces).toEqual([WS2]);
    expect(store.current).toBe('s2');
    expect(store.tabSelected).toEqual([]);
  });
});

describe('guards', () => {
  it('skill_list_changed: an unchanged re-emit is a no-op, a changed list replaces the cache', () => {
    const a: SkillInfo[] = [{ name: 'a', description: '', location: '/x', model_invocation: false }];
    const b: SkillInfo[] = [{ name: 'b', description: '', location: '/y', model_invocation: false }];
    store.skills = { w1: a };
    const beforeRef = store.skills['w1'];
    applyEvents([{ type: 'skill_list_changed', workspace: 'w1', skills: a }]);
    expect(store.skills['w1']).toBe(beforeRef);
    applyEvents([{ type: 'skill_list_changed', workspace: 'w1', skills: b }]);
    expect(store.skills['w1']).not.toBe(beforeRef);
    expect(store.skills['w1']).toEqual(b);
  });


  it('om_status drives the gauge kind; system errors surface', () => {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    applyEvents([{ type: 'om_status', workspace: WS.id, session: 's1', kind: 'observing' }]);
    expect(store.sessions['s1']!.om.kind).toBe('observing');
    applyEvents([
      {
        type: 'system',
        workspace: WS.id,
        session: null,
        kind: { kind: 'error', message: 'boom' }
      }
    ]);
    expect(store.error).toBe('boom');
  });
});

describe('store.error lifecycle', () => {
  it('a successful command clears the banner; an unrelated system event does not', async () => {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    store.current = 's1';
    store.error = 'stale failure';
    applyEvents([
      { type: 'system', workspace: WS.id, session: null, kind: { kind: 'provider_changed' } }
    ]);
    expect(store.error).toBe('stale failure');
    await send('hi', 'steering');
    expect(store.error).toBeNull();
  });
});

describe('subagent_event', () => {
  it('spawned: a mirror entry lands and the child stub is registered', () => {
    store.sessions = openSession({}, 'p', snap('p').snapshot);
    applyEvents([
      {
        type: 'subagent_event',
        workspace: WS.id,
        session: 'p',
        kind: {
          kind: 'spawned',
          handle: 'h1',
          child: 'c',
          agent_type: 'general',
          context_mode: 'fresh',
          title: 'worker'
        }
      }
    ]);
    expect(store.sessions['p']!.subagents).toHaveLength(1);
    expect(store.sessions['p']!.subagents[0]!.state).toBe('running');
    expect(store.sessions['c']).toBeTruthy();
    expect(store.sessions['c']!.state).toBe('running');
    expect(store.sessions['c']!.meta.title).toBe('worker');
    // #247: the child's turn mirrors the spawn — the composer's stop button
    // reads it, and a child has no stream_start of its own.
    expect(store.sessions['c']!.turn).toBe('running');
  });

  it('state: the mirror and the child row update, the declared wait rides the detail', () => {
    store.sessions = openSession({}, 'p', snap('p', { live: { subagents: [sub('c', { state: 'running' })] } }).snapshot);
    applyEvents([
      {
        type: 'subagent_event',
        workspace: WS.id,
        session: 'p',
        kind: { kind: 'state', handle: 'c', child: 'c', state: 'idle', detail: { waiting_on: 'user' }, note: null }
      }
    ]);
    expect(store.sessions['p']!.subagents[0]!.state).toBe('idle');
    expect(store.sessions['p']!.subagents[0]!.waiting_on).toBe('user');
    expect(store.sessions['c']!.state).toBe('idle');
    expect(store.sessions['c']!.waiting_on).toBe('user');
    // #247: the state event's idle closes the child's turn (the stop button
    // flapped on this one before the event drove the turn).
    expect(store.sessions['c']!.turn).toBe('idle');
  });

  it('notified: the wake maps onto a terminal state (stopped must not read back as idle)', () => {
    store.sessions = openSession({}, 'p', snap('p', { live: { subagents: [sub('c', { state: 'running' })] } }).snapshot);
    applyEvents([
      {
        type: 'subagent_event',
        workspace: WS.id,
        session: 'p',
        kind: { kind: 'notified', child: 'c', wake: 'stopped', text: 'done', output: null }
      }
    ]);
    expect(store.sessions['p']!.subagents[0]!.state).toBe('stopped');
    expect(store.sessions['c']!.state).toBe('stopped');
    // #247: a terminal wake leaves the child idle, not running.
    expect(store.sessions['c']!.turn).toBe('idle');
  });

});

describe('plan_changed (the plan pane, spec §5.3)', () => {
  it('a snapshot hydrates the session plan and gate state', () => {
    const s = openSession(
      {},
      's1',
      snap('s1', {
        live: { plan: { steps: [{ text: 'fix the parser', done: true }], criteria: 'build passes', notes: '' } }
      }).snapshot
    );
    expect(s['s1']!.plan.steps).toHaveLength(1);
    expect(s['s1']!.gate_failed).toBe(false);
  });

  it('replaces the session plan: the watcher parse is the pane data', () => {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    applyEvents([
      {
        type: 'plan_changed',
        workspace: WS.id,
        session: 's1',
        plan: {
          steps: [{ text: 'fix the parser', done: false }],
          criteria: 'build passes',
          notes: ''
        }
      }
    ]);
    expect(store.sessions['s1']!.plan.steps).toHaveLength(1);
    expect(store.sessions['s1']!.plan.steps[0]!.done).toBe(false);
  });

  it('an event for an unknown session is a no-op', () => {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    const before = store.sessions['s1']!.plan;
    applyEvents([
      {
        type: 'plan_changed',
        workspace: WS.id,
        session: 'ghost',
        plan: { steps: [{ text: 'x', done: false }], criteria: '', notes: '' }
      }
    ]);
    expect(store.sessions['s1']!.plan).toBe(before);
  });
});

describe('files pane', () => {
  const f = { name: 'x', path: 'x', dir: false, size: 1, ignored: false, git: null };

  it('expand fetches on first expand; collapse drops the listing', async () => {
    store.files = { w1: {} };
    const listed: string[] = [];
    mockIPC((cmd) => {
      if (cmd.type === 'file_list') {
        listed.push(cmd.path);
        return { kind: 'files', files: [f] };
      }
      return { kind: 'none' };
    });
    toggleFileDir('w1', 'src');
    expect(Object.keys(store.files['w1'] ?? {})).toEqual(['src']);
    await vi.waitFor(() => expect(listed).toEqual(['src']));
    toggleFileDir('w1', 'src');
    expect(store.files['w1']).toEqual({});
  });

  it('a change burst coalesces into one wave over the listed dirs only', async () => {
    vi.useFakeTimers();
    store.files = { w1: { '.': [f], src: [f] } };
    const listed: string[] = [];
    mockIPC((cmd) => {
      if (cmd.type === 'file_list') {
        listed.push(cmd.path);
        return { kind: 'files', files: [f] };
      }
      return { kind: 'none' };
    });
    applyEvents([{ type: 'file_tree_changed', workspace: 'w1', changed: ['.', 'src', 'unlisted'] }]);
    expect(listed).toHaveLength(0);
    await vi.advanceTimersByTimeAsync(300);
    expect(listed.sort()).toEqual(['.', 'src']);
    vi.useRealTimers();
  });

  it('a dir collapsed inside the 300 ms window drops out of the wave', async () => {
    vi.useFakeTimers();
    store.files = { w1: { '.': [f], src: [f] } };
    const listed: string[] = [];
    mockIPC((cmd) => {
      if (cmd.type === 'file_list') {
        listed.push(cmd.path);
        return { kind: 'files', files: [f] };
      }
      return { kind: 'none' };
    });
    applyEvents([{ type: 'file_tree_changed', workspace: 'w1', changed: ['src'] }]);
    toggleFileDir('w1', 'src');
    await vi.advanceTimersByTimeAsync(300);
    expect(listed).toHaveLength(0);
    vi.useRealTimers();
  });

  it('a file_list failure records a per-dir error; a successful retry lists and clears it', async () => {
    store.workspaces = [WS];
    store.files = { w1: {} };
    let fail = true;
    mockIPC((cmd) => {
      if (cmd.type === 'file_list') {
        if (fail) throw new Error('permission denied');
        return { kind: 'files', files: [f] };
      }
      return { kind: 'none' };
    });
    toggleFileDir('w1', 'src');
    await vi.waitFor(() => expect(store.fileErrors['w1']?.['src']).toBe('permission denied'));
    fail = false;
    retryDirFetch('w1', 'src');
    await vi.waitFor(() => expect(store.files['w1']!['src']).toEqual([f]));
    expect(store.fileErrors['w1']?.['src']).toBeUndefined();
  });
});
