// The command palette: open, filter, run, close.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup } from '@testing-library/svelte';
import userEvent from '@testing-library/user-event';
import CommandPalette from './CommandPalette.svelte';
import { mockStore as store, switchSession, resetMockStore, seedState } from '../lib/testing/mock-store.svelte';
import type { SessionMeta } from '../lib/protocol';

vi.mock('../lib/store/store.svelte', async () => {
  const m = await import('../lib/testing/mock-store.svelte.ts');
  return { ...m, store: m.mockStore };
});

const meta = (id: string, over: Partial<SessionMeta> = {}): SessionMeta => ({
  id,
  workspace: over.workspace ?? 'w1',
  title: over.title ?? null,
  parent: null,
  created: 1000,
  leaf: null,
  model: null,
  usage: null,
  archived: false,
  ...over
});

beforeEach(() => {
  cleanup();
  resetMockStore({
    current: 'a',
    sessions: {
      a: seedState(meta('a', { title: 'alpha' })),
      b: seedState(meta('b', { title: 'beta', workspace: 'w2' }))
    }
  });
  store.paletteOpen = true;
});

describe('CommandPalette', () => {
  it('renders nothing while closed', () => {
    store.paletteOpen = false;
    render(CommandPalette);
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('lists grouped items with a group label per section', () => {
    render(CommandPalette);
    expect(screen.getByRole('dialog')).toBeTruthy();
    expect(screen.getByText('alpha')).toBeTruthy();
    expect(screen.getByText('sessions')).toBeTruthy();
    expect(screen.getByText('actions')).toBeTruthy();
  });

  it('filters as you type', async () => {
    const user = userEvent.setup();
    render(CommandPalette);
    await user.type(screen.getByPlaceholderText(/jump to a session/), 'beta');
    expect(screen.queryByText('alpha')).toBeNull();
    expect(screen.getByText('beta')).toBeTruthy();
  });

  it('Enter runs the active item (opens the session, closes the palette)', async () => {
    const user = userEvent.setup();
    render(CommandPalette);
    const input = screen.getByPlaceholderText(/jump to a session/);
    await user.type(input, 'beta');
    await user.keyboard('{Enter}');
    expect(store.paletteOpen).toBe(false);
    expect(switchSession).toHaveBeenCalledWith('b');
  });

  it('Esc closes the palette', async () => {
    const user = userEvent.setup();
    render(CommandPalette);
    await user.keyboard('{Escape}');
    expect(store.paletteOpen).toBe(false);
  });
});
