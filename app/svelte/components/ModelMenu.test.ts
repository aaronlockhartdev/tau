// @vitest-environment jsdom
// The model menu: the store's provider rows (#337) grouped by provider, the
// current model dot-highlighted; a pick issues setModel and closes. Opening
// the menu is a fresh read (fetchProviders), never a local first-open cache.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { userEvent } from '@testing-library/user-event';

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => mockInvoke(...args) as Promise<unknown> }));
vi.mock('../lib/store/store.svelte', async () => {
  const m = await import('../lib/testing/mock-store.svelte.ts');
  return { ...m, store: m.mockStore };
});

import type { ProviderInfo, SessionMeta } from '../lib/wire/protocol';
import ModelMenu from './ModelMenu.svelte';
import {
  fetchProviders,
  mockStore,
  resetMockStore,
  seedState,
  setModel
} from '../lib/testing/mock-store.svelte.ts';

const WS = { id: 'w1', name: 'proj', cwd: '/p' };
function meta(id: string, model: string | null = null): SessionMeta {
  return {
    id,
    workspace: WS.id,
    title: null,
    parent: null,
    created: 1000,
    leaf: null,
    model,
    usage: null,
    archived: false
  };
}

const PROVIDERS: ProviderInfo[] = [
  {
    name: 'openrouter',
    base_url: 'https://openrouter.ai/api/v1',
    models: ['dev/qwen3.8', 'anthropic/claude-sonnet']
  },
  { name: 'local', base_url: 'http://localhost:11434/v1', models: ['llama3'] }
];

beforeEach(() => {
  resetMockStore();
  mockStore.sessions = { s1: seedState(meta('s1', 'dev/qwen3.8')) };
  mockStore.current = 's1';
  mockStore.providers = { [WS.id]: PROVIDERS };
});

describe('ModelMenu', () => {
  it('renders the store rows grouped by provider and opens with a fresh read', async () => {
    render(ModelMenu);
    expect(await screen.findByText('openrouter')).toBeInTheDocument();
    expect(screen.getByText('local')).toBeInTheDocument();
    expect(screen.getByText('dev/qwen3.8')).toBeInTheDocument();
    expect(screen.getByText('llama3')).toBeInTheDocument();
    expect(fetchProviders).toHaveBeenCalledWith(WS.id);
  });

  it('marks the current model', async () => {
    render(ModelMenu);
    await screen.findByText('openrouter');
    expect(screen.getByText('current')).toBeInTheDocument();
  });

  it('a pick issues setModel with the qualified name and closes the menu', async () => {
    render(ModelMenu);
    const user = userEvent.setup();
    await user.click(await screen.findByText('llama3'));
    expect(setModel).toHaveBeenCalledWith('local/llama3');
    expect(mockStore.modelMenuOpen).toBe(false);
  });

  it('Escape closes the menu', async () => {
    render(ModelMenu);
    await screen.findByText('openrouter');
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(mockStore.modelMenuOpen).toBe(false);
  });

  it('a scrim click closes the menu', async () => {
    render(ModelMenu);
    await screen.findByText('openrouter');
    await fireEvent.click(document.querySelector('.scrim')!);
    expect(mockStore.modelMenuOpen).toBe(false);
  });

  it('a fetchProviders failure shows the error', async () => {
    fetchProviders.mockRejectedValueOnce(new Error('no providers'));
    render(ModelMenu);
    expect(await screen.findByText('no providers')).toBeInTheDocument();
  });

  it('with no current session the rows are not read', async () => {
    mockStore.current = null;
    render(ModelMenu);
    const loading = await screen.findByText('loading…');
    expect(loading).toBeInTheDocument();
    expect(fetchProviders).not.toHaveBeenCalled();
  });
});
