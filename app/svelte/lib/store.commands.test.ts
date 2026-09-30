// @vitest-environment jsdom
// The store's command surface and small read helpers: send/stop/newSession/
// rename/setModel/deleteQueueItem, pane access, the window title, and the
// dev seam (window.__tau) — the gaps in the boot/events suites.
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }));
const { openFolder } = vi.hoisted(() => ({ openFolder: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => mockInvoke(...args) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: (...args: unknown[]) => openFolder(...args) }));

import { type Command, type CommandOutput, type SessionMeta, type LiveState, type Workspace } from './protocol';
import { openSession } from './sessions';
import {
  applyEvents,
  closeWorkspace,
  deleteQueueItem,
  ensurePane,
  fetchWindow,
  init,
  newSession,
  openWorkspace,
  pane,
  renameSession,
  send,
  setModel,
  store,
  stop,
  switchSession,
  toggleAllReasoning,
  windowTitle
} from './store.svelte';

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

function snap(sid: string, extra: { session?: Partial<SessionMeta>; live?: Partial<LiveState> } = {}) {
  return {
    kind: 'snapshot' as const,
    snapshot: {
      workspace: WS,
      session: meta(sid, WS.id, extra.session),
      entries: [],
      om: { observation_tokens: 0, pending_tokens: 0, reflector_threshold: 0 },
      live: { queue: [], turn: 'idle', subagents: [], tasks: [], ...extra.live },
      cursor: ''
    }
  };
}

function mockIPC(handler: (cmd: Command) => CommandOutput | Promise<CommandOutput>) {
  mockInvoke.mockImplementation(async (_name: unknown, args: { command: Command }) => handler(args.command));
}

function defaultIPC(over: Partial<Record<Command['type'], (cmd: Command) => CommandOutput>> = {}) {
  const base: Record<Command['type'], (cmd: Command) => CommandOutput> = {
    workspace_list: () => ({ kind: 'workspaces', workspaces: [WS] }),
    workspace_open: () => ({ kind: 'workspace', workspace: WS }),
    workspace_close: () => ({ kind: 'none' }),
    session_list: () => ({ kind: 'sessions', sessions: [meta('s1')] }),
    session_new: () => ({ kind: 'session', session: meta('s2') }),
    session_rename: () => ({ kind: 'none' }),
    session_set_model: () => ({ kind: 'none' }),
    session_open: () => snap('s1'),
    session_close: () => ({ kind: 'none' }),
    session_delete: () => ({ kind: 'none' }),
    session_archive: () => ({ kind: 'none' }),
    session_restore: () => ({ kind: 'none' }),
    session_fork: () => ({ kind: 'none' }),
    session_branch: () => ({ kind: 'none' }),
    session_snapshot: () => ({ kind: 'none' }),
    session_entries: () => ({ kind: 'entries', entries: [] }),
    blob_read: () => ({ kind: 'blob', payload: null }),
    message_send: () => ({ kind: 'none' }),
    message_stop: () => ({ kind: 'none' }),
    subagent_types: () => ({ kind: 'agents', agents: [] }),
    subagent_list: () => ({ kind: 'subagents', subagents: [] }),
    subagent_state: () => ({ kind: 'none' }),
    subagent_spawn: () => ({ kind: 'none' }),
    subagent_message: () => ({ kind: 'none' }),
    subagent_stop: () => ({ kind: 'none' }),
    task_create: () => ({ kind: 'none' }),
    task_update: () => ({ kind: 'none' }),
    task_assign: () => ({ kind: 'none' }),
    task_evidence: () => ({ kind: 'none' }),
    task_cancel: () => ({ kind: 'none' }),
    skill_list: () => ({ kind: 'skills', skills: [] }),
    provider_list: () => ({ kind: 'providers', providers: [] }),
    provider_add: () => ({ kind: 'none' }),
    provider_set: () => ({ kind: 'none' }),
    provider_delete: () => ({ kind: 'none' }),
    file_read: () => ({ kind: 'file', file: { text: '', truncated: false } }),
    file_list: () => ({ kind: 'files', files: [] })
  };
  mockIPC((cmd) => (over[cmd.type] ?? base[cmd.type])(cmd));
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
  store.reasoningOpen = true;
  store.modelMenuOpen = false;
  store.entryOpen = new Map();
  (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
}

function oneSession(): void {
  store.sessions = openSession({}, 's1', snap('s1').snapshot);
  store.current = 's1';
}

beforeEach(() => {
  freshStore();
  mockInvoke.mockReset();
  openFolder.mockReset();
  mockInvoke.mockImplementation(async () => ({ kind: 'none' as const }));
});

describe('send / stop guards', () => {
  it('send with no current session is a no-op', async () => {
    await send('hi', 'steering');
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it('send of blank text is a no-op', async () => {
    oneSession();
    await send('   ', 'steering');
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it('send maps the follow-up lane onto the wire follow_up', async () => {
    oneSession();
    await send('hi', 'follow-up');
    const call = mockInvoke.mock.calls.at(-1)![1] as { command: Command };
    expect(call.command).toEqual({ type: 'message_send', session: 's1', text: 'hi', lane: 'follow_up' });
  });

  it('a failed send surfaces the banner and leaves the turn idle', async () => {
    oneSession();
    mockIPC((cmd) => {
      if (cmd.type === 'message_send') throw new Error('provider 500');
      return { kind: 'none' };
    });
    await send('hi', 'steering');
    expect(store.error).toBe('provider 500');
    expect(store.sessions['s1'].turn).toBe('idle');
  });

  it('stop with no current session is a no-op', async () => {
    await stop();
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it('stop issues message_stop and a failure surfaces the banner', async () => {
    oneSession();
    await stop();
    expect(mockInvoke.mock.calls.at(-1)![1]).toEqual({ command: { type: 'message_stop', session: 's1' } });
    mockInvoke.mockRejectedValue(new Error('stop failed'));
    await stop();
    expect(store.error).toBe('stop failed');
  });
});

describe('newSession / renameSession', () => {
  it('newSession opens the fresh session and drops into inline rename', async () => {
    defaultIPC();
    ensurePane(WS.id); // the pane exists in the real app (the left pane creates it)
    const sid = await newSession(WS.id);
    expect(sid).toBe('s2');
    expect(store.current).toBe('s2');
    expect(pane(WS.id)?.renamingId).toBe('s2');
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
    expect(store.sessions['s1'].meta.title).toBe('trimmed');
  });

  it('a failed rename surfaces the banner and keeps the old title', async () => {
    oneSession();
    mockIPC((cmd) => {
      if (cmd.type === 'session_rename') throw new Error('rename failed');
      return { kind: 'none' };
    });
    await renameSession('s1', 'x');
    expect(store.error).toBe('rename failed');
    expect(store.sessions['s1'].meta.title).toBeNull();
  });
});

describe('setModel', () => {
  it('is a no-op with no current session', async () => {
    await setModel('gpt');
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it('is a no-op for an empty or unchanged model', async () => {
    oneSession();
    store.sessions['s1'].meta.model = 'gpt';
    await setModel('');
    await setModel('gpt');
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it('sends session_set_model and updates the row', async () => {
    oneSession();
    defaultIPC();
    await setModel('dev/qwen3.8');
    const call = mockInvoke.mock.calls.at(-1)![1] as { command: Command };
    expect(call.command).toEqual({ type: 'session_set_model', session: 's1', model: 'dev/qwen3.8' });
    expect(store.sessions['s1'].meta.model).toBe('dev/qwen3.8');
  });

  it('a failed setModel surfaces the banner and keeps the old model', async () => {
    oneSession();
    mockIPC((cmd) => {
      if (cmd.type === 'session_set_model') throw new Error('no such model');
      return { kind: 'none' };
    });
    await setModel('dev/nope');
    expect(store.error).toBe('no such model');
    expect(store.sessions['s1'].meta.model).toBeNull();
  });
});

describe('deleteQueueItem', () => {
  it('with no current session is a no-op', async () => {
    await deleteQueueItem('x', 'steering', null, 0);
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it('deletes only the idx-th occurrence of the same (text, lane, source)', async () => {
    oneSession();
    applyEvents([
      {
        type: 'queue',
        workspace: WS.id,
        session: 's1',
        items: [
          { text: 'dup', lane: 'steering' },
          { text: 'dup', lane: 'steering' },
          { text: 'dup', lane: 'follow_up' }
        ]
      }
    ]);
    defaultIPC({
      session_open: () =>
        snap('s1', {
          live: {
            queue: [{ text: 'dup', lane: 'steering' }, { text: 'dup', lane: 'follow_up' }]
          }
        })
    });
    await deleteQueueItem('dup', 'steering', null, 1);
    const pending = store.sessions['s1'].pending;
    expect(pending).toHaveLength(2);
    expect(pending.map((p) => `${p.text}:${p.lane}`)).toEqual(['dup:steering', 'dup:follow-up']);
  });

  it('resyncs the queue from a fresh snapshot', async () => {
    oneSession();
    applyEvents([
      { type: 'queue', workspace: WS.id, session: 's1', items: [{ text: 'a', lane: 'steering' }] }
    ]);
    defaultIPC({
      session_open: () =>
        snap('s1', {
          live: {
            queue: [{ text: 'b', lane: 'force' }, { text: 'a', lane: 'steering', source: 's1-1' }]
          }
        })
    });
    await deleteQueueItem('a', 'steering', null, 0);
    const pending = store.sessions['s1'].pending;
    expect(pending).toHaveLength(2);
    expect(pending[1].source).toBe('s1-1');
  });

  it('a failed resync keeps the optimistic deletion (the queue event will resync)', async () => {
    oneSession();
    applyEvents([
      { type: 'queue', workspace: WS.id, session: 's1', items: [{ text: 'a', lane: 'steering' }] }
    ]);
    mockIPC((cmd) => {
      if (cmd.type === 'session_open') throw new Error('core gone');
      return { kind: 'none' };
    });
    await deleteQueueItem('a', 'steering', null, 0);
    expect(store.sessions['s1'].pending).toHaveLength(0);
    expect(store.error).toBeNull();
  });
});

describe('pane / window title / reasoning', () => {
  it('pane is null for an unknown workspace; ensurePane creates the default', () => {
    expect(pane('w1')).toBeNull();
    const p1 = ensurePane('w1');
    expect(p1.ltab).toBe('sessions');
    // idempotent: a second call keeps the existing pane (state survives)
    p1.ltab = 'files';
    ensurePane('w1');
    expect(pane('w1')?.ltab).toBe('files');
  });

  it('windowTitle is "tau" with no current session', () => {
    expect(windowTitle()).toBe('tau');
  });

  it('windowTitle carries the workspace and session name', () => {
    store.workspaces = [WS];
    store.sessions = openSession({}, 's1', snap('s1', { session: { title: 'alpha' } }).snapshot);
    store.current = 's1';
    expect(windowTitle()).toBe('proj · alpha');
  });

  it('toggleAllReasoning flips the global', () => {
    store.reasoningOpen = true;
    toggleAllReasoning();
    expect(store.reasoningOpen).toBe(false);
    toggleAllReasoning();
    expect(store.reasoningOpen).toBe(true);
  });
});

describe('the open_folder_requested listener (File → Open Folder…)', () => {
  it('a picked directory opens as a workspace', async () => {
    const { listen } = await import('@tauri-apps/api/event');
    const mockListen = vi.mocked(listen);
    let folderHandler: ((e: unknown) => Promise<void>) | null = null;
    mockListen.mockImplementation(async (topic: string, handler: (e: unknown) => Promise<void>) => {
      if (topic === 'open_folder_requested') folderHandler = handler;
      return () => {};
    });
    openFolder.mockResolvedValue('/picked/dir');
    mockIPC((cmd) => {
      switch (cmd.type) {
        case 'workspace_list':
          return { kind: 'workspaces', workspaces: [] };
        case 'workspace_open':
          return { kind: 'workspace', workspace: { id: cmd.cwd, name: 'dir', cwd: cmd.cwd } };
        case 'skill_list':
          return { kind: 'skills', skills: [] };
        case 'session_list':
          return { kind: 'sessions', sessions: [] };
        case 'session_new':
          return { kind: 'session', session: meta('s3', cmd.workspace) };
        case 'session_open':
          return snap('s3', { session: { workspace: '/picked/dir' } });
        default:
          return { kind: 'none' };
      }
    });
    await init();
    expect(folderHandler).toBeTruthy();
    await folderHandler!({});
    await vi.waitFor(() => expect(store.workspaces.some((w) => w.cwd === '/picked/dir')).toBe(true));
  });

  it('a cancelled picker (null) does nothing', async () => {
    const { listen } = await import('@tauri-apps/api/event');
    const mockListen = vi.mocked(listen);
    let folderHandler: ((e: unknown) => Promise<void>) | null = null;
    mockListen.mockImplementation(async (topic: string, handler: (e: unknown) => Promise<void>) => {
      if (topic === 'open_folder_requested') folderHandler = handler;
      return () => {};
    });
    openFolder.mockResolvedValue(null);
    defaultIPC();
    await init();
    const callsBefore = mockInvoke.mock.calls.length;
    await folderHandler!({});
    await new Promise((r) => setTimeout(r, 20));
    expect(mockInvoke.mock.calls.length).toBe(callsBefore);
  });
});

describe('system workspace_opened (syncWorkspaces, live boot rule)', () => {
  it('re-reads the workspace list and opens the first when nothing is open', async () => {
    const WS2: Workspace = { id: 'w2', name: 'other', cwd: '/tmp/other' };
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    store.current = null;
    defaultIPC({
      workspace_list: () => ({ kind: 'workspaces', workspaces: [WS2, WS] }),
      workspace_open: (cmd) =>
        cmd.type === 'workspace_open'
          ? { kind: 'workspace', workspace: cmd.cwd === WS2.cwd ? WS2 : WS }
          : { kind: 'none' },
      session_list: (cmd) =>
        cmd.type === 'session_list'
          ? { kind: 'sessions', sessions: cmd.workspace === WS.id ? [meta('s1')] : [meta('s9', WS2.id)] }
          : { kind: 'sessions', sessions: [] },
      session_open: (cmd) =>
        cmd.type === 'session_open' && cmd.session === 's9'
          ? { ...snap('s9'), snapshot: { ...snap('s9').snapshot, workspace: WS2, session: meta('s9', WS2.id) } }
          : snap('s1')
    });
    applyEvents([
      { type: 'system', workspace: WS.id, session: null, kind: { kind: 'workspace_opened', name: 'other', cwd: '/tmp/other' } }
    ]);
    await vi.waitFor(() => expect(store.current).toBe('s9'));
    expect(store.workspaces.map((w) => w.id).sort()).toEqual(['w1', 'w2']);
  });

  it('does not re-open when a session is already current', async () => {
    store.workspaces = [WS];
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    store.current = 's1';
    defaultIPC();
    applyEvents([
      { type: 'system', workspace: WS.id, session: null, kind: { kind: 'workspace_opened', name: 'x', cwd: '/x' } }
    ]);
    await new Promise((r) => setTimeout(r, 20));
    expect(store.current).toBe('s1');
  });
});

describe('the dev seam (window.__tau)', () => {
  it('exposes applyEvents, the store and the command helpers', async () => {
    const { applyEvents: seamApply, store: seamStore, send: seamSend, stop: seamStop, openWorkspace: seamOpen, closeWorkspace: seamClose, switchSession: seamSwitch, fetchWindow: seamFetch, sessionSetModel } =
      (window as unknown as { __tau: Record<string, unknown> }).__tau;
    expect(seamApply).toBe(applyEvents);
    expect(seamStore()).toBe(store);
    expect(seamSend).toBe(send);
    expect(seamStop).toBe(stop);
    expect(seamOpen).toBe(openWorkspace);
    expect(seamClose).toBe(closeWorkspace);
    expect(seamSwitch).toBe(switchSession);
    expect(seamFetch).toBe(fetchWindow);
    expect(typeof sessionSetModel).toBe('function');
  });

  it('omStatus applies an om_status event for the current session', async () => {
    oneSession();
    const { omStatus } = (window as unknown as { __tau: { omStatus: (k: 'observing' | 'reflecting' | 'idle') => void } })
      .__tau;
    omStatus('observing');
    expect(store.sessions['s1'].om.kind).toBe('observing');
  });

  it('sessionSetModel forwards to the command', async () => {
    oneSession();
    const { sessionSetModel } = (window as unknown as {
      __tau: { sessionSetModel: (session: string, model: string) => Promise<unknown> };
    }).__tau;
    defaultIPC();
    await sessionSetModel('s1', 'dev/qwen3.8');
    const call = mockInvoke.mock.calls.at(-1)![1] as { command: Command };
    expect(call.command).toEqual({ type: 'session_set_model', session: 's1', model: 'dev/qwen3.8' });
  });
});

