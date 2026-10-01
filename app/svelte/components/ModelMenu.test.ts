// @vitest-environment jsdom
// The model menu: fetched from provider_list on open, grouped by provider,
// the current model dot-highlighted; a pick issues setModel and closes.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/svelte';
import { userEvent } from '@testing-library/user-event';
import type { Command, CommandOutput, ProviderInfo, SessionMeta } from '../lib/protocol';
import ModelMenu from './ModelMenu.svelte';
import { mockStore, resetMockStore, setModel, seedState } from '../lib/testing/mock-store.svelte.ts';

const { mockInvoke } = vi.hoisted(() => ({ mockInvoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => mockInvoke(...args) as Promise<unknown> }));
vi.mock('../lib/store.svelte', async () => {
  const m = await import('../lib/testing/mock-store.svelte.ts');
  return { ...m, store: m.mockStore };
});

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

function mockIPC(handler: (cmd: Command) => CommandOutput | Promise<CommandOutput>) {
  mockInvoke.mockImplementation(async (_n: unknown, args: { command: Command }) => handler(args.command));
}

beforeEach(() => {
  resetMockStore();
  mockInvoke.mockReset();
  mockInvoke.mockImplementation(() => Promise.resolve({ kind: 'none' as const }));
  mockStore.sessions = { s1: seedState(meta('s1', 'dev/qwen3.8')) };
  mockStore.current = 's1';
});

describe('ModelMenu', () => {
  it('fetches the providers on open and groups them', async () => {
    mockIPC((cmd) => {
      if (cmd.type === 'provider_list') return { kind: 'providers', providers: PROVIDERS };
      return { kind: 'none' };
    });
    render(ModelMenu);
    expect(await screen.findByText('openrouter')).toBeInTheDocument();
    expect(screen.getByText('local')).toBeInTheDocument();
    expect(screen.getByText('dev/qwen3.8')).toBeInTheDocument();
    expect(screen.getByText('llama3')).toBeInTheDocument();
  });

  it('marks the current model', async () => {
    mockIPC((cmd) => {
      if (cmd.type === 'provider_list') return { kind: 'providers', providers: PROVIDERS };
      return { kind: 'none' };
    });
    render(ModelMenu);
    await screen.findByText('openrouter');
    expect(screen.getByText('current')).toBeInTheDocument();
  });

  it('a pick issues setModel with the qualified name and closes the menu', async () => {
    mockIPC((cmd) => {
      if (cmd.type === 'provider_list') return { kind: 'providers', providers: PROVIDERS };
      return { kind: 'none' };
    });
    render(ModelMenu);
    const user = userEvent.setup();
    await user.click(await screen.findByText('llama3'));
    expect(setModel).toHaveBeenCalledWith('local/llama3');
    expect(mockStore.modelMenuOpen).toBe(false);
  });

  it('Escape closes the menu', async () => {
    mockIPC((cmd) => {
      if (cmd.type === 'provider_list') return { kind: 'providers', providers: PROVIDERS };
      return { kind: 'none' };
    });
    render(ModelMenu);
    await screen.findByText('openrouter');
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(mockStore.modelMenuOpen).toBe(false);
  });

  it('a scrim click closes the menu', async () => {
    mockIPC((cmd) => {
      if (cmd.type === 'provider_list') return { kind: 'providers', providers: PROVIDERS };
      return { kind: 'none' };
    });
    render(ModelMenu);
    await screen.findByText('openrouter');
    await fireEvent.click(document.querySelector('.scrim')!);
    expect(mockStore.modelMenuOpen).toBe(false);
  });

  it('a provider_list failure shows the error', async () => {
    mockInvoke.mockRejectedValue(new Error('no providers'));
    render(ModelMenu);
    expect(await screen.findByText('no providers')).toBeInTheDocument();
  });
});
