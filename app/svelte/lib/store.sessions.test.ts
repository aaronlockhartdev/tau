// @vitest-environment jsdom
// Session archive / restore / permanent-delete convergence (split from
// store.svelte.test.ts to keep both files under the 1000-line CI gate).
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => mockInvoke(...args) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(async () => null) }));

import type { Command, CommandOutput, SessionMeta, Workspace } from './protocol';
import { applySessionList } from './sessions';
import { archiveSession, deleteSession, restoreSession, store } from './store.svelte';

const WS: Workspace = { id: 'w1', name: 'proj', cwd: '/tmp/proj' };

function meta(id: string, workspace = WS.id, extra: Partial<SessionMeta> = {}): SessionMeta {
  return {
    id,
    workspace,
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

function mockIPC(handler: (cmd: Command) => CommandOutput | Promise<CommandOutput>) {
  mockInvoke.mockImplementation(
    async (_name: unknown, args: { command: Command }) => handler(args.command)
  );
}

// Only the routes these tests issue; everything else falls through to a no-op.
function defaultIPC(over: Partial<Record<Command['type'], (cmd: Command) => CommandOutput>> = {}) {
  const base: Partial<Record<Command['type'], (cmd: Command) => CommandOutput>> = {
    session_list: () => ({ kind: 'sessions', sessions: [meta('s1')] }),
    session_archive: (cmd) =>
      cmd.type === 'session_archive'
        ? { kind: 'session', session: meta(cmd.session, WS.id, { archived: true }) }
        : { kind: 'none' },
    session_restore: () => ({ kind: 'none' }),
    session_delete: () => ({ kind: 'none' })
  };
  mockIPC((cmd) => {
    const handler =
      over[cmd.type] ?? base[cmd.type] ?? ((_cmd: Command) => ({ kind: 'none' as const }));
    return handler(cmd);
  });
}

function freshStore() {
  store.workspaces = [];
  store.current = null;
  store.sessions = {};
  store.files = {};
  store.fileErrors = {};
  store.skills = {};
  store.pane = {};
  store.tabSelected = [];
  store.tabSelAnchor = null;
  store.loading = false;
  store.error = null;
  store.tailJump = 0;
  (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
}

beforeEach(() => {
  freshStore();
  mockInvoke.mockReset();
  // A safe no-op default; tests that expect a specific route override it.
  mockInvoke.mockImplementation(async () => ({ kind: 'none' as const }));
  vi.useRealTimers();
});

describe('session archive / restore / delete', () => {
  it('archive: the row keeps its state, meta and flag converge on the command and the refetched list', async () => {
    store.sessions = applySessionList({}, [meta('s1', WS.id, { title: 'one' })]);
    defaultIPC({
      session_archive: () => ({
        kind: 'session',
        session: meta('s1', WS.id, { title: 'one', archived: true })
      }),
      session_list: () => ({
        kind: 'sessions',
        sessions: [meta('s1', WS.id, { title: 'one', archived: true }), meta('s2')]
      })
    });
    await archiveSession('s1');
    expect(store.sessions['s1'].archived).toBe(true);
    expect(store.sessions['s1'].state).toBe('idle');
    expect(store.sessions['s2']).toBeTruthy();
  });

  it('a child session is not archived directly — its parent cascades', async () => {
    store.sessions = applySessionList({}, [meta('s1'), meta('c1', WS.id, { parent: 's1' })]);
    const called: string[] = [];
    defaultIPC({
      session_archive: (cmd) => {
        if (cmd.type === 'session_archive') called.push(cmd.session);
        return { kind: 'none' };
      }
    });
    await archiveSession('c1');
    expect(called).toEqual([]);
    expect(store.sessions['c1'].archived).toBe(false);
  });

  it('restore: the archive flag converges on the refetched list', async () => {
    store.sessions = applySessionList({}, [meta('s1', WS.id, { archived: true })]);
    defaultIPC({
      session_restore: () => ({ kind: 'none' }),
      session_list: () => ({ kind: 'sessions', sessions: [meta('s1', WS.id, { archived: false })] })
    });
    await restoreSession(WS.id, 's1');
    expect(store.sessions['s1'].archived).toBe(false);
  });

  it('delete: the session and its cascade leave the store on the refetch', async () => {
    // The refetch no longer carries the deleted id (nor its child), so both
    // drop out of the store; a survivor stays.
    store.sessions = applySessionList({}, [
      meta('s1', WS.id, { archived: true }),
      meta('c1', WS.id, { parent: 's1', archived: true }),
      meta('s2')
    ]);
    const called: string[] = [];
    defaultIPC({
      session_delete: (cmd) => {
        if (cmd.type === 'session_delete') called.push(cmd.session);
        return { kind: 'none' };
      },
      session_list: () => ({ kind: 'sessions', sessions: [meta('s2')] })
    });
    await deleteSession(WS.id, 's1');
    expect(called).toEqual(['s1']);
    expect(store.sessions['s1']).toBeUndefined();
    expect(store.sessions['c1']).toBeUndefined();
    expect(store.sessions['s2']).toBeTruthy();
  });
});
