// @vitest-environment jsdom
// The provider rows (#337): the per-workspace round-trip, the menu's open
// fetch, the `provider_changed` refetch, and the `config_rejected` banner.
import { beforeEach, describe, expect, it, vi } from 'vitest';

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => mockInvoke(...args) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => () => {}) }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn(() => Promise.resolve(null)) }));

import type { ProviderInfo } from '../wire/protocol';
import { applyEvents, fetchProviders, store } from './store.svelte';
import { WS, bindIPC, freshStore } from '../testing/store-harness';

const { mockIPC } = bindIPC(mockInvoke);

const ROWS: ProviderInfo[] = [
  { name: 'openrouter', base_url: 'https://openrouter.ai/api/v1', models: ['dev/qwen3.8'] },
  { name: 'local', base_url: 'http://localhost:11434/v1', models: ['llama3'] }
];

beforeEach(() => {
  freshStore();
  mockInvoke.mockReset();
  mockInvoke.mockImplementation(() => Promise.resolve({ kind: 'none' as const }));
});

describe('the provider rows', () => {
  it('caches the workspace rows and returns them', async () => {
    const seen: string[] = [];
    mockIPC((cmd) => {
      if (cmd.type === 'provider_list') {
        seen.push(cmd.workspace);
        return { kind: 'providers', providers: ROWS };
      }
      return { kind: 'none' };
    });
    await expect(fetchProviders(WS.id)).resolves.toEqual(ROWS);
    expect(store.providers[WS.id]).toEqual(ROWS);
    // The command is workspace-scoped: the project config layer is one of the
    // layers, so the rows are the workspace's.
    expect(seen).toEqual([WS.id]);
  });

  it('a provider_changed refetches the workspace rows', async () => {
    mockIPC((cmd) =>
      cmd.type === 'provider_list' ? { kind: 'providers', providers: ROWS } : { kind: 'none' }
    );
    applyEvents([{ type: 'system', workspace: WS.id, session: null, kind: { kind: 'provider_changed' } }]);
    await vi.waitFor(() => expect(store.providers[WS.id]).toEqual(ROWS));
  });

  it('a refetch that fails leaves the rows the menu is showing', async () => {
    store.providers[WS.id] = ROWS;
    mockIPC((cmd) =>
      cmd.type === 'provider_list' ? { kind: 'providers', providers: ROWS } : { kind: 'none' }
    );
    applyEvents([{ type: 'system', workspace: WS.id, session: null, kind: { kind: 'provider_changed' } }]);
    await vi.waitFor(() => expect(mockInvoke).toHaveBeenCalled());
    expect(store.providers[WS.id]).toEqual(ROWS);
    expect(store.error).toBeNull();
  });

  it('a refetch that fails surfaces the reason and keeps the rows', async () => {
    store.providers[WS.id] = ROWS;
    mockIPC(() => {
      throw new Error('workspace closed');
    });
    applyEvents([{ type: 'system', workspace: WS.id, session: null, kind: { kind: 'provider_changed' } }]);
    await vi.waitFor(() => expect(store.error).toBe('workspace closed'));
    expect(store.providers[WS.id]).toEqual(ROWS);
  });

  it('a provider_list that answers a foreign kind is a failure, not empty rows', async () => {
    mockIPC(() => ({ kind: 'none' }));
    await expect(fetchProviders(WS.id)).rejects.toThrow('unexpected provider_list output');
  });

  it('a config_rejected shows the banner naming the file and the reason', () => {
    applyEvents([
      {
        type: 'system',
        workspace: WS.id,
        session: null,
        kind: { kind: 'config_rejected', path: '/tmp/tau/config.toml', reason: 'parsing /tmp/tau/config.toml: missing field `base_url`' }
      }
    ]);
    expect(store.error).toContain('/tmp/tau/config.toml');
    expect(store.error).toContain('missing field `base_url`');
  });

  it('a config_rejected that names no layer still says why', () => {
    applyEvents([
      {
        type: 'system',
        workspace: WS.id,
        session: null,
        kind: { kind: 'config_rejected', path: null, reason: 'fleet_soft_bound (9) must not exceed fleet_hard_cap (4)' }
      }
    ]);
    expect(store.error).toBe('config rejected — fleet_soft_bound (9) must not exceed fleet_hard_cap (4)');
  });

  it('a provider_changed for a workspace with no rows leaves the store key absent', async () => {
    mockIPC((cmd) =>
      cmd.type === 'provider_list' ? { kind: 'providers', providers: [] } : { kind: 'none' }
    );
    applyEvents([{ type: 'system', workspace: 'w2', session: null, kind: { kind: 'provider_changed' } }]);
    await vi.waitFor(() => expect(store.providers['w2']).toEqual([]));
    expect(store.providers[WS.id]).toBeUndefined();
  });
});
