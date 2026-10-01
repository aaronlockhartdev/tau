// @vitest-environment jsdom
// The session-command cluster (lib/commands.ts, ticket #42): the F5
// clear-banner → invoke → refetch → reapply dance and the commands spelled
// over it (newSession, rename).
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => mockInvoke(...args) }));

import { type Command, type CommandOutput, type SessionMeta, type Workspace } from './protocol';
import { openSession } from './sessions';
import { store } from './store.svelte';
import {
  newSession,
  refetchSessionList,
  renameSession,
  runSessionCommand,
  switchSession
} from './commands';

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

function snap(
  sid: string,
  extra: { session?: Partial<SessionMeta> } = {}): Extract<CommandOutput, { kind: 'snapshot' }> {
  return {
    kind: 'snapshot' as const,
    snapshot: {
      workspace: WS,
      session: meta(sid, WS.id, extra.session),
      entries: [],
      om: { observation_tokens: 0, pending_tokens: 0, reflector_threshold: 0 },
      live: { queue: [], turn: 'idle', subagents: [], tasks: [] },
      cursor: ''
    }
  };
}

function mockIPC(handler: (cmd: Command) => CommandOutput | Promise<CommandOutput>) {
  mockInvoke.mockImplementation(async (_name: unknown, args: { command: Command }) => handler(args.command));
}

function defaultIPC(over: Partial<Record<Command['type'], (cmd: Command) => CommandOutput>> = {}) {
  const base: Partial<Record<Command['type'], (cmd: Command) => CommandOutput>> = {
    session_list: () => ({ kind: 'sessions', sessions: [meta('s1')] }),
    session_new: () => ({ kind: 'session', session: meta('s2') }),
    session_rename: () => ({ kind: 'none' }),
    session_open: () => snap('s1')
  };
  mockIPC((cmd) => (over[cmd.type] ?? base[cmd.type]!) (cmd));
}

function freshStore() {
  store.workspaces = [];
  store.current = null;
  store.sessions = {};
  store.pane = {};
  store.loading = false;
  store.error = null;
}

function oneSession(): void {
  store.sessions = openSession({}, 's1', snap('s1').snapshot);
  store.current = 's1';
}

beforeEach(() => {
  freshStore();
  mockInvoke.mockReset();
  mockInvoke.mockImplementation(() => Promise.resolve({ kind: 'none' as const }));
});

describe('newSession / renameSession', () => {
  it('newSession opens the fresh session and drops into inline rename', async () => {
    defaultIPC();
    store.pane[WS.id] = {
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
    const sid = await newSession(WS.id);
    expect(sid).toBe('s2');
    expect(store.current).toBe('s2');
    expect(store.pane[WS.id]!.renamingId).toBe('s2');
  });

  it('newSession with an explicit title sends it', async () => {
    defaultIPC();
    await newSession(WS.id, 'my title');
    const calls = mockInvoke.mock.calls.map((c) => (c[1] as { command: Command }).command);
    expect(calls).toContainEqual({ type: 'session_new', workspace: WS.id, title: 'my title' });
  });

  it('a failed newSession surfaces the banner and returns null', async () => {
    defaultIPC({
      session_new: () => {
        throw new Error('no workspace');
      }
    });
    const sid = await newSession(WS.id);
    expect(sid).toBeNull();
    expect(store.error).toBe('no workspace');
  });

  it('renameSession of a blank title is a no-op', async () => {
    oneSession();
    await renameSession('s1', '   ');
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it('renameSession trims, sends, and updates the row title', async () => {
    oneSession();
    defaultIPC();
    await renameSession('s1', '  trimmed  ');
    const call = mockInvoke.mock.calls.at(-1)![1] as { command: Command };
    expect(call.command).toEqual({ type: 'session_rename', session: 's1', title: 'trimmed' });
    expect(store.sessions['s1']!.meta.title).toBe('trimmed');
  });

  it('a failed rename surfaces the banner and keeps the old title', async () => {
    oneSession();
    mockIPC((cmd) => {
      if (cmd.type === 'session_rename') throw new Error('rename failed');
      return { kind: 'none' };
    });
    await renameSession('s1', 'x');
    expect(store.error).toBe('rename failed');
    expect(store.sessions['s1']!.meta.title).toBeNull();
  });
});

describe('session command wrapper (F5)', () => {
  it('on success: clears the banner, invokes, and runs the follow-up', async () => {
    oneSession();
    defaultIPC();
    store.error = 'stale';
    const seen: string[] = [];
    const ok = await runSessionCommand(
      { type: 'session_rename', session: 's1', title: 'x' },
      (out) => {
        seen.push(out.kind);
      }
    );
    expect(ok).toBe(true);
    expect(store.error).toBeNull();
    expect(seen).toEqual(['none']);
  });

  it('on invoke failure: sets the banner and skips the follow-up', async () => {
    oneSession();
    mockIPC((cmd) => {
      if (cmd.type === 'session_rename') throw new Error('boom');
      return { kind: 'none' };
    });
    let ran = false;
    const ok = await runSessionCommand(
      { type: 'session_rename', session: 's1', title: 'x' },
      () => {
        ran = true;
      }
    );
    expect(ok).toBe(false);
    expect(store.error).toBe('boom');
    expect(ran).toBe(false);
  });

  it('refetchSessionList applies the refetched list', async () => {
    oneSession();
    defaultIPC({ session_list: () => ({ kind: 'sessions', sessions: [meta('s9')] }) });
    const seen: string[] = [];
    await refetchSessionList(WS.id, (list) => {
      for (const s of list) seen.push(s.id);
    });
    expect(seen).toEqual(['s9']);
    expect(store.error).toBeNull();
  });

  it('refetchSessionList surfaces the banner on a failed refetch', async () => {
    oneSession();
    mockIPC((cmd) => {
      if (cmd.type === 'session_list') throw new Error('list gone');
      return { kind: 'none' };
    });
    let ran = false;
    await refetchSessionList(WS.id, () => {
      ran = true;
    });
    expect(ran).toBe(false);
    expect(store.error).toBe('list gone');
  });

  it('switchSession rolls current back when the open fails', async () => {
    oneSession();
    mockIPC((cmd) => {
      if (cmd.type === 'session_open') throw new Error('open failed');
      return { kind: 'none' };
    });
    await switchSession('ghost');
    expect(store.current).toBe('s1');
    expect(store.error).toBe('open failed');
  });
});
