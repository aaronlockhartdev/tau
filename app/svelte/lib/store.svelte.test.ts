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
  type Entry,
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
import { applyToolEvent, decodeEntry } from './entries';
import {
  applyEvents,
  archiveSession,
  closeWorkspace,
  fetchWindow,
  init,
  openWorkspace,
  retryDirFetch,
  restoreSession,
  send,
  store,
  switchSession,
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

describe('applyEvents: streaming', () => {
  function oneSession() {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    return store.sessions['s1'];
  }

  it('streams a turn to completion: live fills, the final entry lands with usage', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' },
      { type: 'stream_delta', workspace: WS.id, session: 's1', call_id: 'c1', text: 'hel', reasoning: 'thinking' },
      { type: 'stream_delta', workspace: WS.id, session: 's1', call_id: 'c1', text: 'lo', reasoning: null },
      { type: 'stream_end', workspace: WS.id, session: 's1', call_id: 'c1', interrupted: false, usage: USAGE }
    ]);
    expect(s.live).toHaveLength(0);
    expect(s.turn).toBe('idle');
    expect(s.usage).toEqual(USAGE);
    expect(s.entries).toHaveLength(1);
    const e = s.entries[0];
    expect(e.kind).toBe('message');
    expect(e.id).toBe('c1');
    if (e.kind === 'message' || e.kind === 'interrupted') {
      expect(e.text).toBe('hello');
      expect(e.reasoning).toBe('thinking');
      expect(e.usage).toEqual(USAGE);
    }
  });

  it('marks an interrupted stream as such', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' },
      { type: 'stream_delta', workspace: WS.id, session: 's1', call_id: 'c1', text: 'ab', reasoning: null },
      { type: 'stream_end', workspace: WS.id, session: 's1', call_id: 'c1', interrupted: true, usage: null }
    ]);
    expect(s.entries[0]?.kind).toBe('interrupted');
  });

  it('buffers deltas that land before their start and folds them in', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_delta', workspace: WS.id, session: 's1', call_id: 'c1', text: 'ab', reasoning: null },
      { type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' },
      { type: 'stream_delta', workspace: WS.id, session: 's1', call_id: 'c1', text: 'cd', reasoning: null },
      { type: 'stream_end', workspace: WS.id, session: 's1', call_id: 'c1', interrupted: false, usage: null }
    ]);
    expect(s.entries[0]?.text).toBe('abcd');
  });

  it('closes a stream that ended before we saw its start, out of the buffer', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_delta', workspace: WS.id, session: 's1', call_id: 'c1', text: 'x', reasoning: null },
      { type: 'stream_end', workspace: WS.id, session: 's1', call_id: 'c1', interrupted: false, usage: null }
    ]);
    expect(s.entries).toHaveLength(1);
    expect(s.entries[0]?.id).toBe('c1');
    expect(s.entries[0]?.text).toBe('x');
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

  it('settles live entries before a tool card lands, and ends the tool on tool_end', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' },
      { type: 'stream_delta', workspace: WS.id, session: 's1', call_id: 'c1', text: 'run ls', reasoning: null },
      { type: 'tool_start', workspace: WS.id, session: 's1', call_id: 'c1', tool_call_id: 't1', name: 'bash' },
      { type: 'tool_end', workspace: WS.id, session: 's1', call_id: 'c1', tool_call_id: 't1', name: 'bash', output: 'ok' }
    ]);
    expect(s.entries.map((e) => e.kind)).toEqual(['message', 'tool']);
    const tool = s.entries[1];
    if (tool.kind === 'tool') {
      expect(tool.name).toBe('bash');
      expect(tool.status).toBe('ok');
      expect(tool.output).toBe('ok');
    }
  });

  it('keeps two tools of one call as two cards (keyed on tool_call_id)', () => {
    const s = oneSession();
    applyEvents([
      { type: 'stream_start', workspace: WS.id, session: 's1', call_id: 'c1' },
      { type: 'stream_delta', workspace: WS.id, session: 's1', call_id: 'c1', text: 'x', reasoning: null },
      { type: 'tool_start', workspace: WS.id, session: 's1', call_id: 'c1', tool_call_id: 't1', name: 'a' },
      { type: 'tool_start', workspace: WS.id, session: 's1', call_id: 'c1', tool_call_id: 't2', name: 'b' },
      { type: 'tool_end', workspace: WS.id, session: 's1', call_id: 'c1', tool_call_id: 't1', name: 'a', output: 1 },
      { type: 'tool_end', workspace: WS.id, session: 's1', call_id: 'c1', tool_call_id: 't2', name: 'b', output: 2 }
    ]);
    const tools = s.entries.filter((e) => e.kind === 'tool');
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

  it('send(): the optimistic user bubble lands, the lane maps, the duplicate queues dedupe', async () => {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    store.current = 's1';
    applyEvents([
      { type: 'queue', workspace: WS.id, session: 's1', items: [{ text: 'hi', lane: 'steering' }] }
    ]);
    const before = store.tailJump;
    await send('hi', 'steering');
    expect(store.sessions['s1'].pending).toHaveLength(0);
    const s = store.sessions['s1'];
    const bubble = s.entries[s.entries.length - 1];
    expect(bubble.kind).toBe('user');
    if (bubble.kind === 'user') {
      expect(bubble.text).toBe('hi');
      expect(bubble.id.startsWith('u-')).toBe(true);
    }
    expect(store.tailJump).toBe(before + 1);
    const calls = mockInvoke.mock.calls.map((c) => (c[1] as { command: Command }).command);
    expect(calls.at(-1)).toEqual({ type: 'message_send', session: 's1', text: 'hi', lane: 'steering' });
  });

  it('a failed send splices the optimistic bubble back out (no ghost, no twin on retry)', async () => {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    store.current = 's1';
    defaultIPC({
      message_send: () => {
        throw new Error('provider 500');
      }
    });
    await send('hi', 'steering');
    expect(store.error).toBe('provider 500');
    expect(store.sessions['s1'].entries).toHaveLength(0);
    // a retry of the same text must not stack a second ghost
    await send('hi', 'steering');
    expect(store.sessions['s1'].entries).toHaveLength(0);
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

  it('archive: the row keeps its state, meta and flag converge on the command and the refetched list', async () => {
    store.sessions = applySessionList({}, [meta('s1', WS.id, { title: 'one' })]);
    defaultIPC({
      session_archive: () => ({ kind: 'session', session: meta('s1', WS.id, { title: 'one', archived: true }) }),
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
    store.sessions = applySessionList({}, [meta('s1'), meta('s2', WS2.id)]);
    store.current = 's1';
    mockIPC(() => ({ kind: 'none' }));
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

describe('fetchWindow', () => {
  it('a paged copy arriving mid-turn hydrates the live slot in place (the streamed id is kept)', async () => {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    store.sessions['s1'].live = [{ id: 'c1', kind: 'message', text: 'hello', reasoning: '' }];
    mockIPC((cmd) => {
      if (cmd.type === 'session_entries') {
        return { kind: 'entries', entries: [entryView('42', 'assistant', { text: 'hello', reasoning: 'r' })] };
      }
      return { kind: 'none' };
    });
    await fetchWindow('s1', 0, 50);
    const s = store.sessions['s1'];
    expect(s.entries).toHaveLength(0);
    const l = s.live[0];
    expect(l.id).toBe('c1');
    if (l.kind === 'message' || l.kind === 'interrupted') {
      expect(l.text).toBe('hello');
      expect(l.reasoning).toBe('r');
    }
  });

  it('a first-arriving file copy of an already-settled streamed twin is dropped (the streamed slot is canonical)', async () => {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    store.sessions['s1'].entries.push({ id: 'c1', kind: 'message', text: 'hello', reasoning: '' });
    mockIPC((cmd) => {
      if (cmd.type === 'session_entries') {
        return { kind: 'entries', entries: [entryView('42', 'assistant', { text: 'hello', reasoning: 'r' })] };
      }
      return { kind: 'none' };
    });
    await fetchWindow('s1', 0, 50);
    const s = store.sessions['s1'];
    expect(s.entries).toHaveLength(1);
    expect(s.entries[0].id).toBe('c1');
    const e = s.entries[0];
    if (e.kind === 'message' || e.kind === 'interrupted') {
      expect(e.reasoning).toBe('');
    }
  });

  it('a file entry with no twin appends in arrival order', async () => {
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    mockIPC((cmd) => {
      if (cmd.type === 'session_entries') {
        return { kind: 'entries', entries: [entryView('1', 'user', { text: 'first' })] };
      }
      return { kind: 'none' };
    });
    await fetchWindow('s1', 0, 50);
    expect(store.sessions['s1'].entries[0]?.id).toBe('1');
  });
});

describe('tool failure status (dogfood 2026-09-24: a failed tool showed a check mark)', () => {
  function runningTool(): Entry[] {
    return [{ id: 't1', kind: 'tool', name: 'bash', status: 'running' }];
  }
  function toolEnd(output: unknown) {
    return {
      type: 'tool_end' as const,
      workspace: WS.id,
      session: 's1',
      call_id: 'c1',
      tool_call_id: 't1',
      name: 'bash',
      output
    };
  }
  it('a non-zero exit is an error, a zero exit is ok', () => {
    const failed = applyToolEvent(toolEnd('exit 1\n--- stderr ---\nboom'), runningTool(), []);
    if (failed.entries[0].kind === 'tool') expect(failed.entries[0].status).toBe('error');
    const ok = applyToolEvent(toolEnd('exit 0\n--- stdout ---\nhi'), runningTool(), []);
    if (ok.entries[0].kind === 'tool') expect(ok.entries[0].status).toBe('ok');
  });
  it('a spawn/timeout failure is an error', () => {
    const m = applyToolEvent(toolEnd('bash: timed out after 60s'), runningTool(), []);
    if (m.entries[0].kind === 'tool') expect(m.entries[0].status).toBe('error');
  });
  it('the reload path decodes a persisted failure the same way', () => {
    const e = decodeEntry(entryView('1', 'tool', { name: 'bash', args: { command: 'false' }, output: 'exit 2' }));
    if (e.kind === 'tool') expect(e.status).toBe('error');
    const ok = decodeEntry(entryView('2', 'tool', { name: 'bash', args: {}, output: 'exit 0' }));
    if (ok.kind === 'tool') expect(ok.status).toBe('ok');
  });
});
