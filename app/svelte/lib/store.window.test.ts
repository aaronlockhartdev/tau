// @vitest-environment jsdom
// Windowing / hydration suite: paged reads (fetchWindow), the tab round-trip
// in-flight dedupe, and tool-status decoding — split out of
// store.svelte.test.ts to keep that file under the size gate.
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => mockInvoke(...args) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(async () => null) }));

import type { Command, CommandOutput, Entry, EntryMeta, LiveState, SessionMeta, ViewEntry, Workspace } from './protocol';
import { applySessionList, openSession } from './sessions';
import { applyToolEvent, decodeEntry } from './entries';
import { fetchWindow, openWorkspace, store } from './store.svelte';

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

function mockIPC(handler: (cmd: Command) => CommandOutput | Promise<CommandOutput>) {
  mockInvoke.mockImplementation(async (_name: unknown, args: { command: Command }) => handler(args.command));
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

  it('a blob-backed om entry resolves via blob_read and renders the observation text', async () => {
    // The dogfood shape: a 218KB observation stored as sidecar blob 00000063
    // (payload null in the page read) — the card must show the text, not
    // the [object Object] the null payload decoded to.
    store.sessions = openSession({}, 's1', snap('s1').snapshot);
    const blobView: ViewEntry = {
      id: '00000063',
      parent: null,
      kind: 'om',
      timestamp: 1,
      payload: null,
      blob: { id: '00000063', size: 218_000, hash: '0'.repeat(16) },
      first_kept: null
    };
    mockIPC((cmd) => {
      if (cmd.type === 'session_entries') {
        return { kind: 'entries', entries: [blobView] };
      }
      if (cmd.type === 'blob_read') {
        return {
          kind: 'blob',
          payload: {
            active_observations:
              '<observation-group id="abc" range="1:5">\n* 🔴 (16:21) user set up a workbench\n</observation-group>'
          }
        };
      }
      return { kind: 'none' };
    });
    await fetchWindow('s1', 0, 50);
    const e = store.sessions['s1'].entries[0];
    expect(e?.kind).toBe('om');
    expect(e?.text).toBe('* 🔴 (16:21) user set up a workbench');
    expect(e?.text).not.toContain('[object Object]');
  });
});

describe('workspace tab round trip (switch away and back)', () => {
  const WS2: Workspace = { id: 'w2', name: 'other', cwd: '/tmp/other' };

  function em(id: string, kind: string, preview: string): EntryMeta {
    return { id, parent: null, kind, timestamp: 1, size: 10, preview, first_kept: null };
  }

  it('a paged read in flight during the tab round trip hydrates the re-opened session, not the discarded state', async () => {
    // Longer than the 80-char snapshot preview, so an unhydrated window is
    // visible in the assertion.
    const full = 'x'.repeat(200);
    const preview = full.slice(0, 80) + '…';
    let release: ((v: { kind: 'entries'; entries: ViewEntry[] }) => void) | undefined;
    const hang = new Promise<{ kind: 'entries'; entries: ViewEntry[] }>((r) => {
      release = r;
    });
    store.workspaces = [WS, WS2];
    store.sessions = applySessionList({}, [meta('s1'), meta('s3', WS2.id)]);
    mockIPC((cmd) => {
      switch (cmd.type) {
        case 'workspace_open':
          return { kind: 'workspace', workspace: cmd.cwd === WS.cwd ? WS : WS2 };
        case 'skill_list':
          return { kind: 'skills', skills: [] };
        case 'session_list':
          return cmd.workspace === WS.id
            ? { kind: 'sessions', sessions: [meta('s1')] }
            : { kind: 'sessions', sessions: [meta('s3', WS2.id)] };
        case 'session_open':
          if (cmd.session === 's1')
            return {
              kind: 'snapshot',
              snapshot: {
                ...snap('s1').snapshot,
                entries: [em('1', 'user', 'hello'), em('2', 'assistant', preview), em('3', 'tool', 'exit 0')]
              }
            };
          return snap('s3', { session: { workspace: WS2.id } });
        case 'session_entries':
          return hang;
        case 'file_list':
          return { kind: 'files', files: [] };
        default:
          return { kind: 'none' };
      }
    });
    await openWorkspace(WS);
    expect(store.current).toBe('s1');
    // The transcript's paged read for the open window is in flight when the
    // user switches tabs...
    void fetchWindow('s1', 0, 3);
    await openWorkspace(WS2);
    expect(store.current).toBe('s3');
    // ...and back: s1 re-opens into a fresh state (the snapshot skeleton).
    await openWorkspace(WS);
    expect(store.current).toBe('s1');
    // The remounted transcript requests the identical window: it must dedupe
    // onto the in-flight read, not re-issue it (the perf win stays intact) —
    // proven by the single session_entries call asserted below.
    const p2 = fetchWindow('s1', 0, 3);
    release!({
      kind: 'entries',
      entries: [
        entryView('1', 'user', { text: 'hello', lane: 'force' }),
        entryView('2', 'assistant', { text: full, reasoning: 'r' }),
        entryView('3', 'tool', { name: 'bash', args: { command: 'ls' }, output: 'exit 0' })
      ]
    });
    await p2;
    const entries = mockInvoke.mock.calls.filter(
      (c) => (c[1] as { command: Command }).command.type === 'session_entries'
    );
    expect(entries).toHaveLength(1);
    const s = store.sessions['s1'];
    // Every card kind lands in the re-opened session with its full payload.
    expect(s.entries.map((e) => e.kind)).toEqual(['user', 'message', 'tool']);
    const a = s.entries[1];
    if (a.kind === 'message' || a.kind === 'interrupted') {
      expect(a.text).toBe(full);
      expect(a.reasoning).toBe('r');
    }
    const t = s.entries[2];
    if (t.kind === 'tool') {
      expect(t.name).toBe('bash');
      expect(t.status).toBe('ok');
      expect(t.output).toBe('exit 0');
    }
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
