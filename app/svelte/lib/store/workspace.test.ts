// @vitest-environment jsdom
// The store's workspace suite: session switch's self-heal, the tab close and
// bulk close, the multi-select anchor rules, the event guards, and the
// store.error lifecycle (#320).
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => mockInvoke(...args) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => () => {}) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(() => Promise.resolve(null)) }));
import { type SkillInfo, type Workspace } from '../wire/protocol';
import { applySessionList, openSession, touchChild } from '../rules/sessions';
import {
  applyEvents,
  closeWorkspace,
  closeWorkspaces,
  send,
  store,
  switchSession,
  tabSelect
} from './store.svelte';
import { WS, bindIPC, freshStore, meta, snap, sub } from '../testing/store-harness';

const { mockIPC, defaultIPC } = bindIPC(mockInvoke);
beforeEach(() => {
  freshStore();
  mockInvoke.mockReset();
  // A safe no-op default; tests that expect a specific route override it.
  mockInvoke.mockImplementation(() => Promise.resolve({ kind: 'none' as const }));
  vi.useRealTimers();
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
