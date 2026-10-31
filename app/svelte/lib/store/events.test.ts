// @vitest-environment jsdom
// The store's event suite: the queue's lane mapping, the last_modified
// touch (#282/#283), the subagent mirror, and the plan pane (#320).
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => mockInvoke(...args) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => () => {}) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(() => Promise.resolve(null)) }));
import { type Command } from '../wire/protocol';
import { openSession } from '../rules/sessions';
import { applyEvents, send, store } from './store.svelte';
import { WS, bindIPC, entryView, freshStore, snap, sub } from '../testing/store-harness';

const { defaultIPC } = bindIPC(mockInvoke);
beforeEach(() => {
  freshStore();
  mockInvoke.mockReset();
  // A safe no-op default; tests that expect a specific route override it.
  mockInvoke.mockImplementation(() => Promise.resolve({ kind: 'none' as const }));
  vi.useRealTimers();
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
          { id: 'q1', text: 'a', lane: 'force' },
          { id: 'q2', text: 'b', lane: 'steering' },
          { id: 'q3', text: 'c', lane: 'follow_up' }
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
          { id: 'q1', text: 'a', lane: 'steering' },
          { id: 'q2', text: 'child done', lane: 'steering', source: 's1-1' }
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
      { type: 'queue', workspace: WS.id, session: 's1', items: [{ id: 'q1', text: 'hi', lane: 'steering' }] }
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
    applyEvents([{ type: 'queue', workspace: WS.id, session: 's1', items: [{ id: 'q1', text: 'hi', lane: 'steering' }] }]);
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
