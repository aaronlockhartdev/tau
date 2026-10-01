import { describe, expect, it } from 'vitest';
import {
  applySessionList,
  applySubagentEvent,
  groupIsOpen,
  laneOf,
  makeStub,
  setArchived,
  touchChild,
  windowTitle,
  type SessionMap
} from './sessions';
import type { SessionMeta, SubagentInfo, Workspace } from './protocol';

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
    task: null,
    resume_contract: null,
    ...extra
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

  it('keeps the row mru when correcting an existing child', () => {
    let m: SessionMap = touchChild({}, 'p', 'c', 'running', 100, null, 'x');
    m = touchChild(m, 'p', 'c', 'done', 999, null, null);
    expect(m['c']!.mru).toBe(999);
  });

  it('a new child inherits the parent workspace', () => {
    const m = touchChild({ p: makeStub(meta('p'), null, 'idle', null, 1) }, 'p', 'c', 'running', 5);
    expect(m['c']!.meta.workspace).toBe(WS.id);
    expect(m['c']!.parent).toBe('p');
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
});
