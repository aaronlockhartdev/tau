import { describe, expect, it } from 'vitest';
import {
  applySessionList,
  applySubagentEvent,
  groupIsOpen,
  laneOf,
  makeStub,
  openSession,
  setArchived,
  touchChild,
  windowTitle,
  type SessionMap
} from './sessions';
import type { SessionMeta, Snapshot, SubagentInfo, Workspace } from '../wire/protocol';

const WS: Workspace = { id: 'w1', name: 'proj', cwd: '/tmp/proj' };

function meta(id: string, extra: Partial<SessionMeta> = {}): SessionMeta {
  return {
    id,
    workspace: WS.id,
    title: null,
    parent: null,
    created: 1000,
    leaf: null,
    model: null,
    usage: null,
    archived: false,
    ...extra
  };
}

function sub(child: string, extra: Partial<SubagentInfo> = {}): SubagentInfo {
  return {
    handle: child,
    child,
    agent_type: 'general',
    context_mode: 'fresh',
    state: 'idle',
    waiting_on: null,
    last_message: null,
    usage: null,
    resume_contract: null,
    ...extra
  };
}

// A minimal snapshot: the session meta + a live state. `own_state` and
// `subagents` are the fields #221 exercises; the rest are inert defaults.
function snap(
  sid: string,
  live: Partial<Snapshot['live']> = {},
  metaExtra: Partial<SessionMeta> = {}
): Snapshot {
  return {
    workspace: WS,
    session: meta(sid, metaExtra),
    entries: [],
    om: { observation_tokens: 0, pending_tokens: 0, reflector_threshold: 0 },
    live: {
      queue: [],
      turn: 'idle',
      gate_failed: false,
      plan: { steps: [], criteria: '', notes: '' },
      subagents: [],
      own_state: null,
      ...live
    },
    cursor: ''
  };
}

describe('laneOf', () => {
  it('maps the wire follow_up onto the composer follow-up', () => {
    expect(laneOf('follow_up')).toBe('follow-up');
    expect(laneOf('steering')).toBe('steering');
    expect(laneOf('force')).toBe('force');
  });
});

describe('makeStub', () => {
  it('keeps the declared wait only for an idle child', () => {
    expect(makeStub(meta('c'), 'p', 'idle', 'user', 5).waiting_on).toBe('user');
    expect(makeStub(meta('c'), 'p', 'running', 'user', 5).waiting_on).toBeNull();
  });

  it('derives the turn from the state', () => {
    expect(makeStub(meta('c'), 'p', 'running', null, 5).turn).toBe('running');
    expect(makeStub(meta('c'), 'p', 'done', null, 5).turn).toBe('idle');
  });
});

describe('touchChild', () => {
  it('fills a late spawn title only when the stub has no name', () => {
    let m: SessionMap = touchChild({}, 'p', 'c', 'running', 5, null, null);
    m = touchChild(m, 'p', 'c', 'running', 6, null, 'worker');
    expect(m['c']!.meta.title).toBe('worker');
    m = touchChild(m, 'p', 'c', 'done', 7, null, 'too late');
    expect(m['c']!.meta.title).toBe('worker');
  });

  it('keeps the row last_modified when correcting an existing child', () => {
    let m: SessionMap = touchChild({}, 'p', 'c', 'running', 100, null, 'x');
    m = touchChild(m, 'p', 'c', 'done', 999, null, null);
    expect(m['c']!.last_modified).toBe(999);
  });

  it('a new child inherits the parent workspace', () => {
    const m = touchChild({ p: makeStub(meta('p'), null, 'idle', null, 1) }, 'p', 'c', 'running', 5);
    expect(m['c']!.meta.workspace).toBe(WS.id);
    expect(m['c']!.parent).toBe('p');
  });

  it('derives the child turn from the state (#247: the stop button reads turn)', () => {
    let m: SessionMap = touchChild({}, 'p', 'c', 'running', 5, null, 'w');
    expect(m['c']!.turn).toBe('running');
    // a state correction on the existing row flips the turn with the state
    m = touchChild(m, 'p', 'c', 'idle', 6, 'user', null);
    expect(m['c']!.turn).toBe('idle');
    m = touchChild(m, 'p', 'c', 'running', 7, null, null);
    expect(m['c']!.turn).toBe('running');
    m = touchChild(m, 'p', 'c', 'done', 8, null, null);
    expect(m['c']!.turn).toBe('idle');
  });
});
describe('applySessionList', () => {
  it('an existing row adopts the list archive flag (the list is the authority)', () => {
    const map: SessionMap = { s1: makeStub(meta('s1'), null, 'idle', null, 1) };
    const out = applySessionList(map, [meta('s1', { archived: true })]);
    expect(out['s1']!.archived).toBe(true);
    // an unchanged flag keeps the row reference (no reactive churn)
    const same = applySessionList(out, [meta('s1', { archived: true })]);
    expect(same['s1']).toBe(out['s1']);
  });

  it('a new id materializes as a stub with its list parent', () => {
    const out = applySessionList({}, [meta('c', { parent: 'p' })]);
    expect(out['c']!.parent).toBe('p');
    expect(out['c']!.entries).toEqual({});
  });
});

describe('setArchived', () => {
  it('converges the row on the command meta', () => {
    const map: SessionMap = { s1: makeStub(meta('s1'), null, 'idle', null, 1) };
    const out = setArchived(map, 's1', meta('s1', { archived: true, title: 'renamed' }));
    expect(out['s1']!.archived).toBe(true);
    expect(out['s1']!.meta.title).toBe('renamed');
  });

  it('an unknown session leaves the map alone', () => {
    const map: SessionMap = { s1: makeStub(meta('s1'), null, 'idle', null, 1) };
    expect(setArchived(map, 'nope', meta('nope', { archived: true }))).toBe(map);
  });
});

describe('windowTitle', () => {
  it('is "tau" with no current session', () => {
    expect(windowTitle({ current: null, sessions: {}, workspaces: [WS] })).toBe('tau');
  });

  it('is the workspace name and session id with no title', () => {
    const map: SessionMap = { s1: makeStub(meta('s1'), null, 'idle', null, 1) };
    expect(windowTitle({ current: 's1', sessions: map, workspaces: [WS] })).toBe('proj · s1');
  });

  it('includes the parent title for a child', () => {
    const map: SessionMap = {
      p: makeStub(meta('p', { title: 'parent' }), null, 'idle', null, 1),
      c: makeStub(meta('c', { parent: 'p', title: 'child' }), 'p', 'idle', null, 2)
    };
    expect(windowTitle({ current: 'c', sessions: map, workspaces: [WS] })).toBe('proj · parent › child');
  });

  it('drops the workspace name when it is unknown', () => {
    const map: SessionMap = { s1: makeStub(meta('s1', { workspace: 'gone' }), null, 'idle', null, 1) };
    expect(windowTitle({ current: 's1', sessions: map, workspaces: [] })).toBe('s1');
  });
});

describe('groupIsOpen', () => {
  const active = makeStub(meta('a'), null, 'idle', null, 1);

  it('an explicit toggle set wins', () => {
    expect(groupIsOpen({ openGroups: ['x'] }, active, null, [active])).toBe(false);
    expect(groupIsOpen({ openGroups: ['a'] }, active, null, [active])).toBe(true);
  });

  it('a group with a running sub-agent opens by default', () => {
    const s = { ...active, meta: meta('g'), subagents: [sub('c', { state: 'running' })] };
    expect(groupIsOpen({ openGroups: null }, s, null, [s])).toBe(true);
  });

  it('the active session chain opens by default (grandparent included)', () => {
    const gp = makeStub(meta('gp'), null, 'idle', null, 1);
    const p = makeStub(meta('p', { parent: 'gp' }), 'gp', 'idle', null, 2);
    const a = makeStub(meta('a', { parent: 'p' }), 'p', 'idle', null, 3);
    const all = [gp, p, a];
    const act = all[2]!;
    expect(groupIsOpen({ openGroups: null }, gp, act, all)).toBe(true);
    expect(groupIsOpen({ openGroups: null }, p, act, all)).toBe(true);
  });

  it('an unrelated group stays closed by default', () => {
    const s = makeStub(meta('other'), null, 'idle', null, 1);
    expect(groupIsOpen({ openGroups: null }, s, active, [active, s])).toBe(false);
  });
});

describe('applySubagentEvent (lib level)', () => {
  it('a state event for an unknown handle registers the child (no mirror — the spawn is the mirror source)', () => {
    const map: SessionMap = { p: makeStub(meta('p'), null, 'idle', null, 1) };
    const out = applySubagentEvent(map, 'p', { kind: 'state', handle: 'h', child: 'c', state: 'idle', detail: { waiting_on: 'user' }, note: null }, 7);
    expect(out['c']!.state).toBe('idle');
    expect(out['c']!.waiting_on).toBe('user');
    expect(out['p']!.subagents).toHaveLength(0);
  });

  it('a state event updates the mirror and clears a stale wait for non-idle states', () => {
    let map: SessionMap = { p: makeStub(meta('p'), null, 'idle', null, 1) };
    map = applySubagentEvent(map, 'p', { kind: 'spawned', handle: 'h', child: 'c', agent_type: 'general', context_mode: 'fresh', title: 'w' }, 7);
    map = applySubagentEvent(map, 'p', { kind: 'state', handle: 'h', child: 'c', state: 'idle', detail: { waiting_on: 'user' }, note: null }, 8);
    map = applySubagentEvent(map, 'p', { kind: 'state', handle: 'h', child: 'c', state: 'done', detail: null, note: 'finished' }, 9);
    expect(map['p']!.subagents[0]!.state).toBe('done');
    expect(map['p']!.subagents[0]!.waiting_on).toBeNull();
    expect(map['p']!.subagents[0]!.last_message).toBe('finished');
  });

  it('a notified event with no child row updates the mirror only', () => {
    let map: SessionMap = { p: makeStub(meta('p'), null, 'idle', null, 1) };
    map = applySubagentEvent(map, 'p', { kind: 'spawned', handle: 'h', child: 'c', agent_type: 'general', context_mode: 'fresh', title: 'w' }, 7);
    delete map['c'];
    const out = applySubagentEvent(map, 'p', { kind: 'notified', child: 'c', wake: 'idle', text: 'hi', output: null }, 9);
    expect(out['p']!.subagents[0]!.state).toBe('idle');
    expect(out['p']!.subagents[0]!.waiting_on).toBe('parent');
    expect(out['p']!.subagents[0]!.last_message).toBe('hi');
  });

  it('a notified event on an unknown session leaves the map alone', () => {
    const map: SessionMap = {};
    expect(applySubagentEvent(map, 'ghost', { kind: 'notified', child: 'c', wake: 'idle', text: 'x', output: null }, 9)).toBe(map);
  });

  it('a spawn replaces a duplicate mirror (same handle or child)', () => {
    let map: SessionMap = { p: makeStub(meta('p'), null, 'idle', null, 1) };
    map = applySubagentEvent(map, 'p', { kind: 'spawned', handle: 'h1', child: 'c', agent_type: 'general', context_mode: 'fresh', title: 'one' }, 7);
    map = applySubagentEvent(map, 'p', { kind: 'spawned', handle: 'h2', child: 'c', agent_type: 'general', context_mode: 'fresh', title: 'two' }, 8);
    expect(map['p']!.subagents).toHaveLength(1);
    expect(map['p']!.subagents[0]!.handle).toBe('h2');
  });


  // #226: a re-minted handle (a re-opened parent re-issues "h1" to a NEW
  // child) must not rewrite the row of the child that first owned the
  // handle — the event carries the child's session id, so a handle
  // match alone is not enough to claim a stale row.
  it('a state event that reuses a handle for a new child does not rewrite the old child', () => {
    let map: SessionMap = { p: makeStub(meta('p'), null, 'idle', null, 1) };
    map = applySubagentEvent(map, 'p', { kind: 'spawned', handle: 'h1', child: 'old', agent_type: 'general', context_mode: 'fresh', title: 'one' }, 7);
    map = applySubagentEvent(map, 'p', { kind: 'state', handle: 'h1', child: 'old', state: 'done', detail: null, note: 'finished' }, 8);
    // The re-opened life re-mints h1 for a new child session.
    const out = applySubagentEvent(map, 'p', { kind: 'state', handle: 'h1', child: 'new', state: 'running', detail: null, note: null }, 9);
    // The old child's stub keeps its own identity — no sibling state
    // lands on its card.
    expect(out['old']!.state).toBe('done');
    expect(out['old']!.meta.id).toBe('old');
    expect(out['new']?.state).toBe('running');
    // The roster's h1 row is the new child's (the spawn dedup replaces by
    // handle); the old child is no longer claimed by h1.
    const h1 = out['p']!.subagents.find((x) => x.handle === 'h1');
    expect(h1?.child).toBe('new');
  });
});

describe('openSession (ticket #221 restart restore)', () => {
  it('a snapshot state wins over a stale idle stub (no uniform idle on restart)', () => {
    // A restart materializes every child as an idle stub. Opening the child
    // with a snapshot that says `done` must converge the row on `done`, not
    // preserve the stub's `idle` (the previous behavior).
    const stubbed: SessionMap = { c: makeStub(meta('c', { parent: 'p' }), 'p', 'idle', null, 1) };
    const out = openSession(stubbed, 'c', snap('c', { own_state: { state: 'done', waiting_on: null } }));
    expect(out['c']!.state).toBe('done');
  });

  it('an idle stub with a declared user-wait is corrected by the snapshot', () => {
    // A child parked on the user: the stub carries the wait, the snapshot
    // confirms it — the row keeps both the idle state and the user wait.
    const stubbed: SessionMap = {
      c: makeStub(meta('c', { parent: 'p' }), 'p', 'idle', 'user', 1)
    };
    const out = openSession(
      stubbed,
      'c',
      snap('c', { own_state: { state: 'idle', waiting_on: 'user' } })
    );
    expect(out['c']!.state).toBe('idle');
    expect(out['c']!.waiting_on).toBe('user');
  });

  it('the row last_modified and parent survive the re-open (stable sort, link kept)', () => {
    const stubbed: SessionMap = {
      c: makeStub(meta('c', { parent: 'p' }), 'p', 'idle', null, 42)
    };
    const out = openSession(stubbed, 'c', snap('c', { own_state: { state: 'done', waiting_on: null } }));
    expect(out['c']!.last_modified).toBe(42);
    expect(out['c']!.parent).toBe('p');
  });

  it('a child with no own_state in the snapshot stays idle (no resurrection)', () => {
    // The negative bar: a snapshot carrying no own_state (a child with no
    // recorded state) falls back to the turn-derived state — an idle turn
    // is `idle`, and a stale `done` stub is not resurrected to running.
    const stubbed: SessionMap = { c: makeStub(meta('c', { parent: 'p' }), 'p', 'done', null, 1) };
    const out = openSession(stubbed, 'c', snap('c', {}));
    expect(out['c']!.state).toBe('idle');
    expect(out['c']!.waiting_on).toBeNull();
  });

  it('opening a running child keeps the badge running over a stale idle own_state (#261)', () => {
    // The child's snapshot reports `turn: running` (the live truth) while its
    // recorded own_state is still a stale `idle` from before the turn began.
    // The open must not clobber the row back to `idle`: the running turn wins,
    // and the badge (`state`) and the composer (`turn`) stay in agreement.
    const out = openSession(
      {},
      'c',
      snap('c', { turn: 'running', own_state: { state: 'idle', waiting_on: 'parent' } })
    );
    expect(out['c']!.state).toBe('running');
    expect(out['c']!.turn).toBe('running');
  });

  it('a running snapshot state is not synthesized from a parked stub', () => {
    // before a restart (impossible in practice, but the rule is the point)
    // is reset to the snapshot's truth, never kept as a ghost running state.
    const stubbed: SessionMap = { c: makeStub(meta('c', { parent: 'p' }), 'p', 'running', null, 1) };
    const out = openSession(
      stubbed,
      'c',
      snap('c', { own_state: { state: 'idle', waiting_on: 'parent' } })
    );
    expect(out['c']!.state).toBe('idle');
  });

  it('a root session (no own_state) derives its state from the turn', () => {
    const out = openSession({}, 's1', snap('s1', { turn: 'running' }));
    expect(out['s1']!.state).toBe('running');
    const idle = openSession({}, 's1', snap('s1', { turn: 'idle' }));
    expect(idle['s1']!.state).toBe('idle');
  });

  it('keeps wire-only entries absent from the snapshot (#248: the switch must not drop them)', () => {
    // A session switched away from mid-stream: the in-memory map holds a
    // wire-only streaming assistant entry (not yet file-backed) plus a file
    // entry the snapshot also lists. Re-opening must keep the wire-only one
    // and let the snapshot win the id it lists — the previous replace dropped
    // both.
    const base = snap('s1');
    const snapWithFile = { ...base, entries: [{ id: 'e1', parent: null, kind: 'user', timestamp: 1, size: 10, preview: 'file preview', first_kept: null }] };
    const prev: SessionMap = {
      s1: {
        ...makeStub(meta('s1'), null, 'running', null, 1),
        entries: {
          e1: { id: 'e1', kind: 'user', text: 'stale in-memory' },
          // wire-only: a streaming assistant entry the snapshot does not list
          stream1: { id: 'stream1', kind: 'message', text: 'in-flight…' }
        }
      }
    };
    const out = openSession(prev, 's1', snapWithFile);
    // the wire-only entry survives
    expect(out['s1']!.entries['stream1']).toEqual({ id: 'stream1', kind: 'message', text: 'in-flight…' });
    // the id the snapshot lists takes the snapshot's decoded value
    expect(out['s1']!.entries['e1']).toMatchObject({ id: 'e1', text: 'file preview' });
  });
});
