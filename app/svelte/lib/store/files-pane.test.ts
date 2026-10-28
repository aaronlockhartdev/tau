// @vitest-environment jsdom
// The files pane suite: expand/collapse listing, the watcher's coalesced
// refetch wave, and a dir's per-path fetch error (#320).
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => mockInvoke(...args) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => () => {}) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(() => Promise.resolve(null)) }));
import { applyEvents, retryDirFetch, store, toggleFileDir } from './store.svelte';
import { WS, bindIPC, freshStore } from '../testing/store-harness';

const { mockIPC } = bindIPC(mockInvoke);
beforeEach(() => {
  freshStore();
  mockInvoke.mockReset();
  // A safe no-op default; tests that expect a specific route override it.
  mockInvoke.mockImplementation(() => Promise.resolve({ kind: 'none' as const }));
  vi.useRealTimers();
});

describe('files pane', () => {
  const f = { name: 'x', path: 'x', dir: false, size: 1, ignored: false, git: null };

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
    expect(Object.keys(store.files['w1'] ?? {})).toEqual(['src']);
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
    await vi.waitFor(() => expect(store.files['w1']!['src']).toEqual([f]));
    expect(store.fileErrors['w1']?.['src']).toBeUndefined();
  });
});
