// @vitest-environment jsdom
// Boot suite: init's workspace/session convergence and the open's in-flight
// dedupe (#320).
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => mockInvoke(...args) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => () => {}) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(() => Promise.resolve(null)) }));
import { type Command, type Workspace } from '../wire/protocol';
import { init, openWorkspace, store } from './store.svelte';
import { WS, bindIPC, freshStore, meta, snap } from '../testing/store-harness';

const { mockIPC, defaultIPC } = bindIPC(mockInvoke);
beforeEach(() => {
  freshStore();
  mockInvoke.mockReset();
  // A safe no-op default; tests that expect a specific route override it.
  mockInvoke.mockImplementation(() => Promise.resolve({ kind: 'none' as const }));
  vi.useRealTimers();
});

describe('boot (init)', () => {
  it('opens the first listed session from its snapshot', async () => {
    defaultIPC();
    await init();
    expect(store.workspaces).toEqual([WS]);
    expect(store.current).toBe('s1');
    const s = store.sessions['s1']!;
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
