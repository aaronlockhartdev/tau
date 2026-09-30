// @vitest-environment jsdom
// Store-level suite (roadmap 1c): boot, applyEvents (streaming, tools,
// queueing, sub-agent mirror), session switch + archive convergence, and
// the refetch-wave guards — the browser rig's checks, at node speed.
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => mockInvoke(...args) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(async () => null) }));

import {
  type Command,
  type CommandOutput,
  type LiveState,
  type SessionMeta,
  type SkillInfo,
  type SubagentInfo,
  type Task,
  type Usage,
  type ViewEntry,
  type Workspace
} from './protocol';
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

function snap(sid: string, extra: { session?: Partial<SessionMeta>; live?: Partial<LiveState> } = {}): {
  kind: 'snapshot';
  snapshot: import('./protocol').Snapshot;
} {
  return {
    kind: 'snapshot',
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

function entryView(id: string, kind: string, payload: Record<string, unknown>): ViewEntry {
  return {
    id,
    parent: null,
    kind,
    timestamp: 1,
    payload,
    blob: null,
    first_kept: null
  };
}


const USAGE: Usage = { input_tokens: 5, output_tokens: 10, total_tokens: 15, cached_prompt_tokens: 0 };

// A sub-agent mirror row, complete (the wire shape the events carry).
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

function mockIPC(handler: (cmd: Command) => CommandOutput | Promise<CommandOutput>) {
  mockInvoke.mockImplementation(async (_name: unknown, args: { command: Command }) => handler(args.command));
}

// The default route table: the commands a boot/switch flow issues.
function defaultIPC(over: Partial<Record<Command['type'], (cmd: Command) => CommandOutput>> = {}) {
  const base: Record<Command['type'], (cmd: Command) => CommandOutput> = {
    workspace_list: () => ({ kind: 'workspaces', workspaces: [WS] }),
    workspace_open: () => ({ kind: 'workspace', workspace: WS }),
    workspace_close: () => ({ kind: 'none' }),
    session_list: () => ({ kind: 'sessions', sessions: [meta('s1')] }),
    session_new: () => ({ kind: 'session', session: meta('s2') }),
    session_rename: () => ({ kind: 'none' }),
    session_set_model: () => ({ kind: 'none' }),
    session_open: () => ({ kind: 'snapshot', snapshot: snap('s1').snapshot }),
    session_close: () => ({ kind: 'none' }),
    session_delete: () => ({ kind: 'none' }),
    session_archive: (cmd) =>
      cmd.type === 'session_archive' ? { kind: 'session', session: meta(cmd.session, WS.id, { archived: true }) } : { kind: 'none' },
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
    subagent_state: () => ({ kind: 'subagent', subagent: sub('c1') }),
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
  (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
}

beforeEach(() => {
  freshStore();
  mockInvoke.mockReset();
  // A safe no-op default; tests that expect a specific route override it.
  mockInvoke.mockImplementation(async () => ({ kind: 'none' as const }));
  vi.useRealTimers();
});

describe('boot (init)', () => {
  it('opens the first listed session from its snapshot', async () => {
    defaultIPC();
    await init();
    expect(store.workspaces).toEqual([WS]);
    expect(store.current).toBe('s1');
    const s = store.sessions['s1'];
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
    return store.sessions['s1'];
  }
  function upsert(id: string, kind: string, payload: Record<string, unknown>) {
    return {
      type: 'entry_upsert' as const,
      workspace: WS.id,
      session: 's1',
      entry: entryView(id, kind, payload)
    };
  }

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
    const e = s.entries['00000001'];
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

  it('computes TPS over the call duration', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(0);
    const s = oneSession();
    applyEvents([{ type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' }]);
    await vi.advanceTimersByTimeAsync(1000);
    applyEvents([
      { type: 'stream_end', workspace: WS.id, session: 's1', call_id: 'c1', interrupted: false, usage: USAGE }
    ]);
    expect(s.tps).toBeCloseTo(10);
    vi.useRealTimers();
  });

  it('a tool call is one card: the call upsert starts it, the result upsert fills the output', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' },
      upsert('00000001', 'assistant', { text: 'run ls', interrupted: false }),
      upsert('00000002', 'tool', { call_id: 't1', name: 'bash', args: { command: 'ls' } }),
      upsert('00000002', 'tool', { call_id: 't1', name: 'bash', args: { command: 'ls' }, output: 'ok' }),
      { type: 'stream_end', workspace: WS.id, session: 's1', call_id: 'c1', interrupted: false, usage: null }
    ]);
    const tools = Object.values(s.entries).filter((e) => e.kind === 'tool');
    expect(tools).toHaveLength(1);
    const tool = tools[0];
    if (tool.kind === 'tool') {
      expect(tool.name).toBe('bash');
      expect(tool.status).toBe('ok');
      expect(tool.output).toBe('ok');
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
    expect(store.sessions['s1'].pending.map((p) => p.lane)).toEqual(['force', 'steering', 'follow-up']);
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
    const p = store.sessions['s1'].pending;
    expect(p).toHaveLength(2);
    expect(p[0].source).toBeUndefined();
    expect(p[1].source).toBe('s1-1');
  });

  it('send(): the user card lands on the core upsert echo (no optimistic card), the lane maps, the duplicate queues dedupe', async () => {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    store.current = 's1';
    applyEvents([
      { type: 'queue', workspace: WS.id, session: 's1', items: [{ text: 'hi', lane: 'steering' }] }
    ]);
    const before = store.tailJump;
    await send('hi', 'steering');
    expect(store.sessions['s1'].pending).toHaveLength(0);
    // ADR-0008: no client-minted card — the core's echo is the card.
    expect(Object.keys(store.sessions['s1'].entries)).toEqual([]);
    applyEvents([
      {
        type: 'entry_upsert',
        workspace: WS.id,
        session: 's1',
        entry: entryView('00000001', 'user', { text: 'hi', lane: 'steering' })
      }
    ]);
    const card = store.sessions['s1'].entries['00000001'];
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
    expect(Object.keys(store.sessions['s1'].entries)).toEqual([]);
    // a retry of the same text must not stack a second ghost
    await send('hi', 'steering');
    expect(Object.keys(store.sessions['s1'].entries)).toEqual([]);
  });
});

describe('session switch', () => {
  it('replaces the stub with the snapshot and keeps the parent link', async () => {
    store.sessions = applySessionList({}, [meta('p'), meta('c', WS.id, { parent: 'p' })]);
    defaultIPC({ session_open: () => snap('c', { session: { title: 'child' } }) });
    await switchSession('c');
    const s = store.sessions['c'];
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

  it('re-opening a session preserves the row mru (no tree reshuffle away from the clicked row)', async () => {
    store.sessions = applySessionList({}, [meta('s1')]);
    store.sessions['s1'].mru = 9999;
    defaultIPC();
    await switchSession('s1');
    expect(store.sessions['s1'].mru).toBe(9999);
  });

  it('self-heals the child stubs from the snapshot (fills a lost spawn, corrects a stale one, keeps the mru)', async () => {
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
    expect(store.sessions['c'].state).toBe('done');
    expect(store.sessions['c'].mru).toBe(5000);
    expect(store.sessions['d']).toBeTruthy();
    expect(store.sessions['d'].state).toBe('running');
    expect(store.sessions['d'].parent).toBe('p');
  });

  it('synthesizes the tab entry for a disk-restored child with no live supervisor', async () => {
    store.sessions = touchChild(applySessionList({}, [meta('p')]), 'p', 'c', 'done', 1, 'parent');
    defaultIPC({ session_open: () => snap('p') });
    await switchSession('p');
    expect(store.sessions['p'].subagents).toEqual([sub('c', { state: 'done' })]);
  });

  it('a branch_move event moves the leaf', () => {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    applyEvents([
      { type: 'session_event', workspace: WS.id, session: 's1', kind: { kind: 'branch_move', leaf: '7' } }
    ]);
    expect(store.sessions['s1'].meta.leaf).toBe('7');
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

  it('task_changed: an unchanged re-emit is a no-op, a change replaces', () => {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    const tasks: Task[] = [];
    store.sessions['s1'].tasks = tasks;
    const beforeRef = store.sessions['s1'].tasks;
    applyEvents([{ type: 'task_changed', workspace: WS.id, session: 's1', tasks }]);
    expect(store.sessions['s1'].tasks).toBe(beforeRef);
    const other: Task[] = [
      {
        id: 't1',
        title: 'x',
        status: 'pending',
        steps: [],
        criteria: [],
        evidence: [],
        blockers: [],
        decisions: [],
        notes: [],
        updated: 1
      }
    ];
    applyEvents([{ type: 'task_changed', workspace: WS.id, session: 's1', tasks: other }]);
    expect(store.sessions['s1'].tasks).not.toBe(beforeRef);
    expect(store.sessions['s1'].tasks).toEqual(other);
  });

  it('om_status drives the gauge kind; system errors surface', () => {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    applyEvents([{ type: 'om_status', workspace: WS.id, session: 's1', kind: 'observing' }]);
    expect(store.sessions['s1'].om.kind).toBe('observing');
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
    expect(store.sessions['p'].subagents).toHaveLength(1);
    expect(store.sessions['p'].subagents[0].state).toBe('running');
    expect(store.sessions['c']).toBeTruthy();
    expect(store.sessions['c'].state).toBe('running');
    expect(store.sessions['c'].meta.title).toBe('worker');
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
    expect(store.sessions['p'].subagents[0].state).toBe('idle');
    expect(store.sessions['p'].subagents[0].waiting_on).toBe('user');
    expect(store.sessions['c'].state).toBe('idle');
    expect(store.sessions['c'].waiting_on).toBe('user');
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
    expect(store.sessions['p'].subagents[0].state).toBe('stopped');
    expect(store.sessions['c'].state).toBe('stopped');
  });

  it('task_changed: a child-targeted emission fills the child pane (the projection)', () => {
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
    const projected: Task[] = [
      {
        id: 't1',
        title: 'x',
        status: 'in_progress',
        steps: [],
        criteria: [],
        evidence: [],
        blockers: [],
        decisions: [],
        notes: [],
        worker: { session: 'c', status: 'in_progress' },
        updated: 1
      }
    ];
    applyEvents([{ type: 'task_changed', workspace: WS.id, session: 'c', tasks: projected }]);
    expect(store.sessions['c'].tasks).toEqual(projected);
  });
});

describe('files pane', () => {
  const f = { name: 'x', path: 'x', dir: false, size: 1 };

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
    expect(Object.keys(store.files['w1'])).toEqual(['src']);
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
    await vi.waitFor(() => expect(store.files['w1']['src']).toEqual([f]));
    expect(store.fileErrors['w1']?.['src']).toBeUndefined();
  });
});
